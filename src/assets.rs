use crate::geometry::{Hit, Ray, EPSILON};
use crate::math::Vec3;
use crate::scene::Result;
use image::{ImageReader, Limits};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::sync::OnceLock;

const MAX_OBJ: u64 = 16 * 1024 * 1024;
const MAX_VERTICES: usize = 80_000;
const MAX_TRIANGLES: usize = 100_000;
const MAX_IMAGE_SIDE: u32 = 512;

#[derive(Serialize, Deserialize)]
pub struct Mesh {
    vertices: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
    #[serde(skip)]
    tree: OnceLock<Tree>,
}

impl Mesh {
    pub fn import(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        if file.metadata()?.len() > MAX_OBJ {
            return Err("OBJ exceeds the 16 MiB input limit".into());
        }
        let mut vertices = Vec::new();
        let mut triangles = Vec::new();
        for (line, text) in BufReader::new(file).lines().enumerate() {
            let text = text?;
            let mut words = text
                .split('#')
                .next()
                .unwrap_or_default()
                .split_ascii_whitespace();
            match words.next() {
                Some("v") => {
                    let parse = |s: Option<&str>| {
                        s.and_then(|s| s.parse::<f32>().ok())
                            .filter(|n| n.is_finite() && n.abs() <= 1e12)
                    };
                    let coordinates = [
                        parse(words.next()),
                        parse(words.next()),
                        parse(words.next()),
                    ];
                    let [Some(x), Some(y), Some(z)] = coordinates else {
                        return Err(format!("invalid vertex on OBJ line {}", line + 1).into());
                    };
                    vertices.push(Vec3(x, y, z));
                    if vertices.len() > MAX_VERTICES {
                        return Err("OBJ exceeds the 80,000-vertex limit".into());
                    }
                }
                Some("f") => {
                    let count = vertices.len() as i32;
                    let index = |s: &str| {
                        let i = s.split('/').next()?.parse::<i32>().ok()?;
                        let i = if i < 0 { count + i } else { i.checked_sub(1)? };
                        (0..count).contains(&i).then_some(i as u32)
                    };
                    let face: Vec<u32> = words
                        .map(index)
                        .collect::<Option<_>>()
                        .filter(|face: &Vec<_>| face.len() >= 3)
                        .ok_or_else(|| format!("invalid face on OBJ line {}", line + 1))?;
                    if triangles.len() + face.len() - 2 > MAX_TRIANGLES {
                        return Err("OBJ exceeds the 100,000-triangle limit".into());
                    }
                    triangles.extend(face[1..].windows(2).map(|pair| [face[0], pair[0], pair[1]]));
                }
                _ => {}
            }
        }
        if vertices.len() < 3 || triangles.is_empty() {
            return Err("OBJ does not contain a mesh".into());
        }
        let bounds = Bounds::from_points(vertices.iter().copied());
        let center = (bounds.min + bounds.max) * 0.5;
        let radius = (bounds.max - bounds.min).max_component() * 0.5;
        if radius <= 0.0 {
            return Err("OBJ has no spatial extent".into());
        }
        let vertices = vertices
            .into_iter()
            .map(|v| {
                let Vec3(x, y, z) = (v - center) / radius;
                [x, y, z].map(|c| c.clamp(-1.0, 1.0))
            })
            .collect();
        Ok(Self {
            vertices,
            triangles,
            tree: OnceLock::new(),
        })
    }

    pub fn valid(&self) -> bool {
        (3..=MAX_VERTICES).contains(&self.vertices.len())
            && (1..=MAX_TRIANGLES).contains(&self.triangles.len())
            && self
                .vertices
                .iter()
                .flatten()
                .all(|c| (-1.0..=1.0).contains(c))
            && self
                .triangles
                .iter()
                .flatten()
                .all(|&index| (index as usize) < self.vertices.len())
    }

    pub fn units(&self) -> usize {
        self.vertices.len() + self.triangles.len()
    }

    fn vertex(&self, index: u32) -> Vec3 {
        let [x, y, z] = self.vertices[index as usize];
        Vec3(x, y, z)
    }

    pub fn hit(&self, ray: Ray, limit: f32, smooth: bool) -> Option<Hit> {
        let tree = self.tree.get_or_init(|| Tree::new(self));
        let mut best = None;
        tree.root.hit(self, tree, ray, smooth, &mut best, limit);
        best
    }
}

struct Tree {
    root: Node,
    order: Vec<usize>,
    normals: Vec<Vec3>,
}

impl Tree {
    fn new(mesh: &Mesh) -> Self {
        let mut normals = vec![Vec3::default(); mesh.vertices.len()];
        let centers: Vec<_> = mesh
            .triangles
            .iter()
            .map(|&indices| {
                let [a, b, c] = indices.map(|index| mesh.vertex(index));
                let normal = (b - a).cross(c - a);
                for index in indices {
                    normals[index as usize] = normals[index as usize] + normal;
                }
                (a + b + c) / 3.0
            })
            .collect();
        let mut order: Vec<_> = (0..mesh.triangles.len()).collect();
        let root = Node::build(mesh, &centers, &mut order, 0);
        Self {
            root,
            order,
            normals,
        }
    }
}

struct Node {
    bounds: Bounds,
    children: Option<Box<[Node; 2]>>,
    range: std::ops::Range<usize>,
}

impl Node {
    fn build(mesh: &Mesh, centers: &[Vec3], order: &mut [usize], start: usize) -> Self {
        let bounds = Bounds::from_points(
            order
                .iter()
                .flat_map(|&i| mesh.triangles[i].map(|index| mesh.vertex(index))),
        );
        let children = if order.len() > 8 {
            let axis = (bounds.max - bounds.min).longest_axis();
            let half = order.len() / 2;
            order.select_nth_unstable_by(half, |&a, &b| {
                centers[a].axis(axis).total_cmp(&centers[b].axis(axis))
            });
            let (left, right) = order.split_at_mut(half);
            Some(Box::new([
                Self::build(mesh, centers, left, start),
                Self::build(mesh, centers, right, start + half),
            ]))
        } else {
            None
        };
        Self {
            bounds,
            children,
            range: start..start + order.len(),
        }
    }

    fn hit(
        &self,
        mesh: &Mesh,
        tree: &Tree,
        ray: Ray,
        smooth: bool,
        best: &mut Option<Hit>,
        limit: f32,
    ) {
        if !self
            .bounds
            .intersects(ray, best.map_or(limit, |hit| hit.distance))
        {
            return;
        }
        if let Some(children) = &self.children {
            children[0].hit(mesh, tree, ray, smooth, best, limit);
            children[1].hit(mesh, tree, ray, smooth, best, limit);
            return;
        }
        for &i in &tree.order[self.range.clone()] {
            let vertices = mesh.triangles[i];
            let [a, b, c] = vertices.map(|index| mesh.vertex(index));
            let (ab, ac) = (b - a, c - a);
            let p = ray.direction.cross(ac);
            let determinant = ab.dot(p);
            if determinant.abs() < 1e-9 {
                continue;
            }
            let inverse = determinant.recip();
            let s = ray.origin - a;
            let u = s.dot(p) * inverse;
            if !(0.0..=1.0).contains(&u) {
                continue;
            }
            let q = s.cross(ab);
            let v = ray.direction.dot(q) * inverse;
            let distance = ac.dot(q) * inverse;
            if v < 0.0
                || u + v > 1.0
                || !(EPSILON..best.map_or(limit, |hit| hit.distance)).contains(&distance)
            {
                continue;
            }
            let face = ab.cross(ac);
            let normal = if smooth {
                let [na, nb, nc] = vertices.map(|index| tree.normals[index as usize]);
                let n = na * (1.0 - u - v) + nb * u + nc * v;
                if n.dot(n) > 1e-18 {
                    n
                } else {
                    face
                }
            } else {
                face
            };
            *best = Some(Hit {
                distance,
                normal,
                local: ray.at(distance),
            });
        }
    }
}

struct Bounds {
    min: Vec3,
    max: Vec3,
}

impl Bounds {
    fn from_points(points: impl IntoIterator<Item = Vec3>) -> Self {
        let mut min = Vec3(f32::INFINITY, f32::INFINITY, f32::INFINITY);
        let mut max = min * -1.0;
        for Vec3(x, y, z) in points {
            min = Vec3(min.0.min(x), min.1.min(y), min.2.min(z));
            max = Vec3(max.0.max(x), max.1.max(y), max.2.max(z));
        }
        Self { min, max }
    }

    fn intersects(&self, ray: Ray, mut far: f32) -> bool {
        let mut near = EPSILON;
        for i in 0..3 {
            let (o, d) = (ray.origin.axis(i), ray.direction.axis(i));
            let (a, b) = (self.min.axis(i), self.max.axis(i));
            if d.abs() < 1e-9 {
                if !(a..=b).contains(&o) {
                    return false;
                }
                continue;
            }
            let (u, v) = ((a - o) / d, (b - o) / d);
            near = near.max(u.min(v));
            far = far.min(u.max(v));
            if near > far {
                return false;
            }
        }
        true
    }
}

#[derive(Serialize, Deserialize)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pixels: Vec<[u8; 3]>,
}

impl Bitmap {
    pub fn import(path: &Path) -> Result<Self> {
        let mut reader = ImageReader::open(path)?.with_guessed_format()?;
        let mut limits = Limits::default();
        limits.max_image_width = Some(8192);
        limits.max_image_height = Some(8192);
        limits.max_alloc = Some(192 * 1024 * 1024);
        reader.limits(limits);
        let image = reader.decode()?;
        let image = if image.width().max(image.height()) > MAX_IMAGE_SIDE {
            image.resize(
                MAX_IMAGE_SIDE,
                MAX_IMAGE_SIDE,
                image::imageops::FilterType::Triangle,
            )
        } else {
            image
        }
        .into_rgb8();
        let (width, height) = image.dimensions();
        let pixels = image.pixels().map(|p| p.0).collect();
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    pub fn valid(&self) -> bool {
        (1..=MAX_IMAGE_SIDE).contains(&self.width)
            && (1..=MAX_IMAGE_SIDE).contains(&self.height)
            && self.pixels.len() == self.width as usize * self.height as usize
    }

    pub fn units(&self) -> usize {
        self.pixels.len()
    }

    pub fn color(&self, x: f32, y: f32, back: bool) -> [f32; 3] {
        let u = if back { -x } else { x };
        let u = (0.5 + u * 0.5).clamp(0.0, 1.0);
        let v = (0.5 - y * 0.5).clamp(0.0, 1.0);
        let col = (u * (self.width - 1) as f32).round() as usize;
        let row = (v * (self.height - 1) as f32).round() as usize;
        self.pixels[row * self.width as usize + col].map(|value| value as f32 / 255.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::Object;
    use crate::render;
    use crate::scene::{Preset, Scene};
    use image::{Rgb, RgbImage};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn scene_embeds_obj_and_bitmap_and_reopens_for_render() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("terrain-test-{}-{unique}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let file = dir.join("geometry.obj");
        fs::write(&file, "v -1 -1 0\nv 1 -1 0\nv 0 1 0\nf -3 -2 -1\n").unwrap();
        let path = dir.join("image.png");
        RgbImage::from_pixel(12, 8, Rgb([210, 25, 160]))
            .save(&path)
            .unwrap();
        let mut scene = Preset::Checker.scene(42);
        scene.objects = vec![
            Object::mesh("Triangle", Mesh::import(&file).unwrap()),
            Object::picture("Photo", Bitmap::import(&path).unwrap())
                .at([0.45, 0.60, 0.12], [0.16, 0.12, 0.005]),
        ];
        let project = dir.join("saved.terrain");
        scene.save(&project).unwrap();
        fs::remove_file(&file).unwrap();
        fs::remove_file(&path).unwrap();

        let saved = Scene::open(&project).unwrap();
        let triangle = saved.objects[0].mesh.as_ref().unwrap();
        let hit = triangle.hit(
            Ray {
                origin: Vec3(0.0, 0.0, 2.0),
                direction: Vec3(0.0, 0.0, -1.0),
            },
            10.0,
            true,
        );
        assert!((hit.unwrap().distance - 2.0).abs() < 1e-5);
        let bitmap = saved.objects[1].bitmap.as_ref().unwrap();
        assert_eq!(
            bitmap.color(0.0, 0.0, false),
            [210.0 / 255.0, 25.0 / 255.0, 160.0 / 255.0]
        );
        assert_eq!(
            render::render(&saved, 64, 64, false),
            render::render(&scene, 64, 64, false)
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
