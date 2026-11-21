use crate::geometry::{Heightfield, Hit, Placed, Ray, EPSILON};
use crate::math::{fractal, noise, smooth, Vec3};
use crate::objects::{linear, Material, Pattern, Shape};
use crate::scene::{atomic_write, Look, Result, Scene};
use image::{ImageBuffer, RgbImage};
use std::path::Path;
use std::sync::Mutex;

const FAR: f32 = 80.0;
const UP: Vec3 = Vec3(0.0, 1.0, 0.0);

pub fn dimensions(width: u32, height: u32) -> Result<()> {
    if !(16..=4096).contains(&width) || !(16..=4096).contains(&height) {
        return Err("image dimensions must each be between 16 and 4096".into());
    }
    Ok(())
}

pub fn render(scene: &Scene, width: u32, height: u32, antialias: bool) -> RgbImage {
    let world = World::new(scene);
    let camera = Projection::new(scene, width as f32 / height as f32);
    let stride = width as usize * 3;
    let mut pixels = vec![0; height as usize * stride];
    let workers = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(12);
    let rows = (height as usize).div_ceil(workers * 8).clamp(1, 16);
    let samples: &[(f32, f32)] = if antialias {
        &[(0.32, 0.13), (0.85, 0.32), (0.15, 0.68), (0.68, 0.85)]
    } else {
        &[(0.5, 0.5)]
    };

    let work = Mutex::new(pixels.chunks_mut(rows * stride).enumerate());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let (world, camera, work) = (&world, &camera, &work);
            scope.spawn(move || loop {
                let next = work.lock().expect("render work queue").next();
                let Some((chunk_index, chunk)) = next else {
                    break;
                };
                for (row, scanline) in chunk.chunks_exact_mut(stride).enumerate() {
                    let y = (chunk_index * rows + row) as f32;
                    for (x, pixel) in scanline.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                        let mut color = Vec3::default();
                        for (sx, sy) in samples {
                            let ray = camera
                                .ray((x as f32 + sx) / width as f32, (y + sy) / height as f32);
                            color = color + world.trace(ray, 2);
                        }
                        *pixel = to_rgb(color / samples.len() as f32);
                    }
                }
            });
        }
    });
    ImageBuffer::from_raw(width, height, pixels).expect("rendered buffer has the correct size")
}

pub fn pick(scene: &Scene, aspect: f32, x: f32, y: f32) -> Option<usize> {
    let world = World::new(scene);
    let ray = Projection::new(scene, aspect).ray(x, y);
    match world.closest(ray)? {
        (_, Surface::Object(index)) => Some(index),
        _ => None,
    }
}

struct Projection {
    eye: Vec3,
    forward: Vec3,
    right: Vec3,
    up: Vec3,
    shift: [f32; 2],
}

impl Projection {
    fn new(scene: &Scene, aspect: f32) -> Self {
        let camera = &scene.camera;
        let target = scene.camera_target();
        let eye = target + direction(camera.heading, camera.elevation) * camera.distance;
        let forward = (target - eye).normalized();
        let right = forward.cross(UP).normalized();
        let up = right.cross(forward);
        let span = (camera.field_of_view.to_radians() * 0.5).tan();
        let (right, up, shift) = if let Some(lens) = &camera.lens {
            let (sin, cos) = lens.roll.to_radians().sin_cos();
            let tilted_right = right * cos + up * sin;
            let tilted_up = up * cos - right * sin;
            (
                tilted_right * (span * lens.scale[0]),
                tilted_up * (span * lens.scale[1] / aspect),
                lens.shift,
            )
        } else {
            (right * (span * aspect), up * span, [0.0; 2])
        };
        Self {
            eye,
            forward,
            right,
            up,
            shift,
        }
    }

    fn ray(&self, x: f32, y: f32) -> Ray {
        Ray {
            origin: self.eye,
            direction: (self.forward
                + self.right * (2.0 * x - 1.0 + 2.0 * self.shift[0])
                + self.up * (1.0 - 2.0 * y + 2.0 * self.shift[1]))
                .normalized(),
        }
    }
}

pub fn save_png(path: &Path, image: &RgbImage) -> Result<()> {
    atomic_write(path, |file| {
        image.write_to(file, image::ImageFormat::Png)?;
        Ok(())
    })
}

fn direction(heading: f32, elevation: f32) -> Vec3 {
    let (x, z) = heading.to_radians().sin_cos();
    let (y, flat) = elevation.to_radians().sin_cos();
    Vec3(x * flat, y, z * flat)
}

fn to_rgb(Vec3(r, g, b): Vec3) -> [u8; 3] {
    [r, g, b].map(|channel| {
        let x = channel.max(0.0);
        let tone = if x < 0.75 {
            x
        } else {
            0.75 + 0.25 * (1.0 - (-4.0 * (x - 0.75)).exp())
        };
        let srgb = if tone <= 0.0031308 {
            tone * 12.92
        } else {
            1.055 * tone.powf(1.0 / 2.4) - 0.055
        };
        (srgb.clamp(0.0, 1.0) * 255.0).round() as u8
    })
}

#[derive(Clone, Copy)]
enum Surface {
    Terrain,
    Object(usize),
    Water,
}

struct World<'a> {
    scene: &'a Scene,
    terrain: Option<Heightfield<'a>>,
    objects: Vec<Placed<'a>>,
    materials: Vec<PreparedMaterial>,
    terrain_material: Option<PreparedMaterial>,
    water_material: PreparedMaterial,
    sun: Vec3,
    sunlight: Vec3,
    zenith: Vec3,
    horizon: Vec3,
}

impl<'a> World<'a> {
    fn new(scene: &'a Scene) -> Self {
        let sky = &scene.atmosphere;
        let sun = direction(sky.sun_heading, sky.sun_elevation);
        let warmth = sky.warmth;
        Self {
            scene,
            terrain: scene
                .terrain_visible
                .then(|| Heightfield::new(&scene.terrain, scene.relief)),
            objects: scene.objects.iter().map(Placed::new).collect(),
            materials: scene
                .objects
                .iter()
                .map(|object| PreparedMaterial::from(&object.material))
                .collect(),
            terrain_material: scene.terrain_color.as_ref().map(PreparedMaterial::from),
            water_material: PreparedMaterial::from(&scene.water.material),
            sun,
            sunlight: Vec3(1.0, 0.96, 0.90).mix(Vec3(1.0, 0.58, 0.32), warmth),
            zenith: linear(sky.zenith),
            horizon: linear(sky.horizon),
        }
    }

    fn closest(&self, ray: Ray) -> Option<(Hit, Surface)> {
        let water = &self.scene.water;
        let distance = (water.level - ray.origin.1) / ray.direction.1;
        let plane =
            water.enabled && ray.direction.1.abs() > 0.000001 && (EPSILON..FAR).contains(&distance);
        // Leave room for terrain intersection tolerances at the waterline.
        let bound = if plane {
            (distance + EPSILON).min(FAR)
        } else {
            FAR
        };
        let solid = self.solid(ray, bound);
        if plane && solid.is_none_or(|(hit, _)| distance < hit.distance) {
            Some((
                Hit {
                    distance,
                    normal: UP,
                    local: ray.at(distance),
                },
                Surface::Water,
            ))
        } else {
            solid
        }
    }

    fn trace(&self, ray: Ray, bounces: u32) -> Vec3 {
        let water = &self.scene.water;
        let Some((hit, surface)) = self.closest(ray) else {
            return self.sky(ray);
        };
        let p = ray.at(hit.distance);
        let (color, normal, appearance) = match surface {
            Surface::Object(index) => {
                let object = self.objects[index].object;
                let material = &self.materials[index];
                let color = if let Some(bitmap) = &object.bitmap {
                    linear(bitmap.color(hit.local.0, hit.local.1, hit.local.2 < 0.0))
                } else {
                    let mut p = hit.local;
                    if object.shape == Shape::Box && object.material.pattern == Pattern::Checker {
                        p.1 *= object.scale[1] / object.scale[0].max(object.scale[2]);
                    }
                    if object.natural_terrain && object.shape == Shape::Terrain {
                        self.earth(p, hit.normal, p.1)
                    } else {
                        pigment(material, p, Some(object.shape), self.scene.seed)
                    }
                };
                (color, hit.normal, material.finish)
            }
            Surface::Terrain => match &self.terrain_material {
                Some(material) => (
                    pigment(material, p, None, self.scene.seed),
                    hit.normal,
                    material.finish,
                ),
                None => (
                    self.earth(p, hit.normal, p.1 / self.scene.relief),
                    hit.normal,
                    Finish {
                        reflection: 0.025,
                        roughness: 0.88,
                        metallic: 0.0,
                    },
                ),
            },
            Surface::Water => {
                let r = water.ripples * 0.065;
                let w = p.0 * 85.0 + p.2 * 53.0;
                let v = p.0 * -37.0 + p.2 * 148.0;
                let ripple = Vec3(
                    r * (w.sin() * 0.65 + v.cos() * 0.35),
                    1.0,
                    r * (v.sin() * 0.62 + w.cos() * 0.38),
                )
                .normalized();
                let normal = UP
                    .mix(ripple, 1.0 - smooth(1.4, 12.0, hit.distance))
                    .normalized();
                let color = pigment(&self.water_material, p, None, self.scene.seed);
                (
                    color,
                    normal,
                    Finish {
                        reflection: water.mirror,
                        roughness: 0.08 + 0.13 * water.ripples,
                        metallic: 0.0,
                    },
                )
            }
        };

        let normal = if normal.dot(ray.direction) < 0.0 {
            normal
        } else {
            normal * -1.0
        };
        let reflect_ray = Ray {
            origin: p + normal * (EPSILON * 3.0),
            direction: ray.direction.reflect(normal),
        };
        let lambert = normal.dot(self.sun).max(0.0);
        let lit = lambert > 0.0
            && self
                .solid(
                    Ray {
                        origin: p + normal * (EPSILON * 3.0),
                        direction: self.sun,
                    },
                    FAR,
                )
                .is_none();
        let ambient = 0.14 + 0.21 * smooth(-0.30, 1.0, normal.1);
        let diffuse = component(
            color,
            Vec3(0.77, 0.83, 1.12) * ambient
                + self.sunlight * (if lit { lambert * 1.37 } else { 0.0 }),
        );
        let cosine = (-normal.dot(ray.direction)).clamp(0.0, 1.0);
        let fresnel = (1.0 - cosine).powi(4);
        let reflection = appearance.reflection * (0.49 + 0.50 * fresnel)
            + (1.0 - appearance.reflection) * 0.10 * fresnel;
        let reflection = reflection * (1.0 - appearance.roughness * 0.45);
        let specular = self
            .sun
            .dot(reflect_ray.direction)
            .max(0.0)
            .powf(12.0 + 220.0 * (1.0 - appearance.roughness).powi(3));
        let specular_tint = self
            .sunlight
            .mix(component(self.sunlight, color), appearance.metallic * 0.76);
        let sparkle = specular_tint
            * if lit {
                specular * (1.0 - 0.76 * appearance.roughness) * 1.10
            } else {
                0.0
            };
        let reflected = if reflection > 0.045 {
            let incoming = if bounces == 0 {
                self.sky(reflect_ray)
            } else {
                self.trace(reflect_ray, bounces - 1)
            };
            incoming.mix(component(incoming, color), appearance.metallic * 0.58)
        } else {
            Vec3::default()
        };
        let mut result = diffuse * (1.0 - reflection) + reflected * reflection + sparkle;
        if matches!(surface, Surface::Water) {
            result = result + color * 0.11;
        }

        let haze = self.scene.atmosphere.haze;
        let amount = 1.0 - (-hit.distance * (0.013 + haze * haze * 0.75)).exp();
        result.mix(self.horizon, amount)
    }

    fn solid(&self, ray: Ray, max: f32) -> Option<(Hit, Surface)> {
        let mut best = None;
        let mut end = max;
        for (index, object) in self.objects.iter().enumerate() {
            if let Some(hit) = object.hit(ray, end) {
                end = hit.distance;
                best = Some((hit, Surface::Object(index)));
            }
        }
        if let Some(hit) = self
            .terrain
            .as_ref()
            .and_then(|terrain| terrain.hit(ray, end))
        {
            best = Some((hit, Surface::Terrain));
        }
        best
    }

    fn sky(&self, ray: Ray) -> Vec3 {
        let atmosphere = &self.scene.atmosphere;
        let altitude = ray.direction.1;
        let mut background = self
            .horizon
            .mix(self.zenith, smooth(0.0, 0.73, altitude.max(0.0).sqrt()));
        let facing = self.sun.dot(ray.direction).max(0.0);
        let warmth = atmosphere.warmth;
        let glow = Vec3(1.0, 0.40, 0.19) * (facing.powf(9.0) * warmth.powi(3) * 0.36);
        background = background + glow;
        if altitude > 0.012 && atmosphere.clouds > 0.08 {
            let scale = 4.0 / (altitude + 0.25);
            let px = (ray.origin.0 + ray.direction.0 * scale) * 0.75;
            let pz = (ray.origin.2 + ray.direction.2 * scale) * 1.12;
            let seed = self.scene.seed ^ 147;
            let warp = noise(px * 0.68, pz * 0.54, seed + 1) - 0.5;
            let cloud = fractal(px + warp * 1.8, pz, seed, 5);
            let threshold = 0.77 - 0.33 * atmosphere.clouds;
            let cover =
                smooth(threshold - 0.05, threshold + 0.075, cloud) * smooth(0.012, 0.105, altitude);
            let cloud_color = Vec3(0.26, 0.31, 0.49).mix(
                Vec3(0.97, 0.96, 0.98),
                smooth(threshold, threshold + 0.19, cloud),
            );
            let cloud_color = cloud_color.mix(
                Vec3(0.95, 0.51, 0.30),
                warmth.powi(3) * facing.powf(2.0) * 0.72,
            );
            background = background.mix(cloud_color, cover * 0.93);
        }
        if altitude > 0.0 {
            background = background + self.sunlight * (smooth(0.99960, 0.99980, facing) * 3.4);
        }
        background
    }

    fn earth(&self, p: Vec3, normal: Vec3, h: f32) -> Vec3 {
        let i = self.scene.seed ^ 8917;
        let detail = fractal(p.0 * 82.0, p.2 * 82.0, i, 4);
        let broad = fractal(p.0 * 15.0, p.2 * 15.0, i, 3);
        let slope = normal.1.max(0.0);
        let (base, high) = match self.scene.look {
            Look::Alpine => {
                let vegetation = Vec3(0.041, 0.082, 0.026).mix(Vec3(0.14, 0.21, 0.072), detail);
                let rock = Vec3(0.079, 0.067, 0.066).mix(Vec3(0.33, 0.31, 0.27), detail);
                let amount =
                    (1.0 - smooth(0.34, 0.78, slope)) * 0.75 + 0.25 * smooth(0.30, 0.62, h);
                (vegetation.mix(rock, amount), Vec3(0.83, 0.93, 1.0))
            }
            Look::Canyon => {
                let bands = smooth(-0.35, 0.55, (p.1 * 133.0 + broad * 3.0).sin());
                (
                    Vec3(0.25, 0.063, 0.027)
                        .mix(Vec3(0.75, 0.30, 0.119), bands * 0.65 + detail * 0.35),
                    Vec3(0.97, 0.74, 0.39),
                )
            }
            Look::Coast => {
                let ash = Vec3(0.028, 0.025, 0.027).mix(Vec3(0.205, 0.13, 0.092), detail);
                (
                    ash.mix(
                        Vec3(0.075, 0.142, 0.045),
                        smooth(0.55, 0.92, slope) * smooth(0.28, 0.7, broad),
                    ),
                    Vec3(0.58, 0.58, 0.65),
                )
            }
        };
        let snow = smooth(
            self.scene.snowline - 0.025,
            self.scene.snowline + 0.1,
            h + (broad - 0.5) * 0.25,
        ) * smooth(0.28, 0.77, slope);
        base.mix(high, snow)
    }
}

#[derive(Clone, Copy)]
struct Finish {
    reflection: f32,
    roughness: f32,
    metallic: f32,
}

impl From<&Material> for Finish {
    fn from(material: &Material) -> Self {
        Self {
            reflection: material.reflectivity,
            roughness: material.roughness,
            metallic: material.metallic,
        }
    }
}

fn component(a: Vec3, b: Vec3) -> Vec3 {
    Vec3(a.0 * b.0, a.1 * b.1, a.2 * b.2)
}

struct PreparedMaterial {
    pattern: Pattern,
    color: Vec3,
    accent: Vec3,
    density: f32,
    finish: Finish,
}

impl From<&Material> for PreparedMaterial {
    fn from(material: &Material) -> Self {
        Self {
            pattern: material.pattern,
            color: linear(material.color),
            accent: linear(material.accent),
            density: material.density,
            finish: Finish::from(material),
        }
    }
}

fn pigment(material: &PreparedMaterial, p: Vec3, shape: Option<Shape>, seed: u32) -> Vec3 {
    let f = material.density;
    let Vec3(x, y, z) = p * f;
    let blend = match material.pattern {
        Pattern::Solid => return material.color,
        Pattern::Checker => {
            let (u, v, w) = if shape == Some(Shape::Sphere) {
                let turns = p.2.atan2(p.0) / std::f32::consts::TAU + 0.5;
                let latitude = p.1.clamp(-1.0, 1.0).asin() / std::f32::consts::PI + 0.5;
                (turns * f.round() * 4.0, latitude * f.round() * 2.0, 0.0)
            } else if shape == Some(Shape::Box) && p.1.abs() < 0.20 {
                (x + 0.003, z + 0.003, 0.0)
            } else {
                (x + 0.003, z + 0.003, y + 0.003)
            };
            ((u.floor() as i32 + v.floor() as i32 + w.floor() as i32) & 1) as f32
        }
        Pattern::Marble => {
            let twist = fractal(x + y * 0.37, z + y * 0.91, seed ^ 3498, 5);
            let fine = noise(x * 2.7 + y, z * 2.1 - y, seed ^ 323);
            let ridge =
                ((x * 1.3 + y * 0.80 + z * 0.68 + 11.5 * twist).sin() * 0.5 + 0.5).powf(4.5);
            smooth(0.36, 0.85, ridge * 0.83 + fine * 0.26)
        }
        Pattern::Lava => {
            let q = fractal(x + 0.8 * y, z - 0.55 * y, seed ^ 175, 5);
            1.0 - smooth(0.017, 0.10, (q - 0.54).abs())
        }
        Pattern::Ripple => {
            let q = fractal(x + y, z, seed ^ 441, 4);
            smooth(0.18, 0.81, ((x * 3.9 + z + q * 9.2).sin() + 1.0) * 0.5)
        }
    };
    material.color.mix(material.accent, blend)
}
