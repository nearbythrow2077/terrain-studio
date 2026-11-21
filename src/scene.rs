use crate::math::{fractal, noise, smooth, Vec3};
use crate::objects::{Material, Object, Pattern, Shape};
use image::{DynamicImage, ImageReader, Limits};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Cursor, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
pub const DEFAULT_GRID: usize = 193;
pub const MAX_OBJECTS: usize = 256;
const MAX_GRID: usize = 513;
const MAX_PROJECT: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Look {
    Alpine,
    Canyon,
    Coast,
}

impl Look {
    pub const ALL: [Self; 3] = [Self::Alpine, Self::Canyon, Self::Coast];

    pub fn name(self) -> &'static str {
        match self {
            Self::Alpine => "Alpine lake",
            Self::Canyon => "Painted canyon",
            Self::Coast => "Volcanic coast",
        }
    }

    pub fn from_slug(slug: &str) -> Option<Self> {
        match slug {
            "alpine" => Some(Self::Alpine),
            "canyon" => Some(Self::Canyon),
            "coast" => Some(Self::Coast),
            _ => None,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Terrain {
    pub width: usize,
    pub depth: usize,
    pub heights: Vec<f32>,
}

impl Terrain {
    pub fn generate(look: Look, seed: u32) -> Self {
        let size = DEFAULT_GRID;
        let mut heights = Vec::with_capacity(size * size);
        for row in 0..size {
            for col in 0..size {
                let x = col as f32 / (size - 1) as f32 * 2.0 - 1.0;
                let z = row as f32 / (size - 1) as f32 * 2.0 - 1.0;
                heights.push(landform(look, seed, x, z));
            }
        }
        Self {
            width: size,
            depth: size,
            heights,
        }
    }

    pub fn import(path: &Path) -> Result<Self> {
        Self::decode(ImageReader::new(BufReader::new(File::open(path)?)).with_guessed_format()?)
    }

    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        Self::decode(ImageReader::new(Cursor::new(data)).with_guessed_format()?)
    }

    fn decode<R: io::BufRead + io::Seek>(mut reader: ImageReader<R>) -> Result<Self> {
        let mut limits = Limits::default();
        limits.max_image_width = Some(8192);
        limits.max_image_height = Some(8192);
        limits.max_alloc = Some(192 * 1024 * 1024);
        reader.limits(limits);
        Ok(Self::from_luma16(reader.decode()?.into_luma16()))
    }

    pub fn from_luma16(samples: image::ImageBuffer<image::Luma<u16>, Vec<u16>>) -> Self {
        let ratio = (MAX_GRID as f32 / samples.width().max(samples.height()) as f32).min(1.0);
        let width = ((samples.width() as f32 * ratio).round() as u32).max(2);
        let depth = ((samples.height() as f32 * ratio).round() as u32).max(2);
        let map = if (width, depth) == samples.dimensions() {
            samples
        } else {
            DynamicImage::ImageLuma16(samples)
                .resize_exact(width, depth, image::imageops::FilterType::Triangle)
                .into_luma16()
        };
        let heights = map
            .into_raw()
            .into_iter()
            .map(|v| v as f32 / 65535.0)
            .collect();
        Self {
            width: width as usize,
            depth: depth as usize,
            heights,
        }
    }

    pub fn brush(&mut self, u: f32, v: f32, radius: f32, amount: f32) {
        let w = (self.width - 1) as f32;
        let d = (self.depth - 1) as f32;
        let left = ((u - radius) * w).floor().max(0.0) as usize;
        let right = ((u + radius) * w).ceil().min(w) as usize;
        let top = ((v - radius) * d).floor().max(0.0) as usize;
        let bottom = ((v + radius) * d).ceil().min(d) as usize;
        for row in top..=bottom {
            for col in left..=right {
                let distance = (col as f32 / w - u).hypot(row as f32 / d - v);
                let weight = 1.0 - smooth(0.0, radius, distance);
                let sample = &mut self.heights[row * self.width + col];
                *sample = (*sample + amount * weight).clamp(0.0, 1.0);
            }
        }
    }

    pub fn smooth(&mut self) {
        let original = self.heights.clone();
        for row in 0..self.depth {
            for col in 0..self.width {
                let i = row * self.width + col;
                let left = row * self.width + col.saturating_sub(1);
                let right = row * self.width + (col + 1).min(self.width - 1);
                let up = row.saturating_sub(1) * self.width + col;
                let down = (row + 1).min(self.depth - 1) * self.width + col;
                self.heights[i] = (original[i] * 4.0
                    + original[left]
                    + original[right]
                    + original[up]
                    + original[down])
                    / 8.0;
            }
        }
    }

    pub fn save_map(&self, path: &Path) -> Result<()> {
        let pixels: Vec<u16> = self
            .heights
            .iter()
            .map(|h| (h * 65535.0).round() as u16)
            .collect();
        let map = image::ImageBuffer::from_raw(self.width as u32, self.depth as u32, pixels)
            .ok_or("invalid heightmap dimensions")?;
        atomic_write(path, |file| {
            DynamicImage::ImageLuma16(map).write_to(file, image::ImageFormat::Png)?;
            Ok(())
        })
    }

    pub fn validate(&self) -> bool {
        (2..=MAX_GRID).contains(&self.width)
            && (2..=MAX_GRID).contains(&self.depth)
            && self.heights.len() == self.width * self.depth
            && self.heights.iter().all(|h| (0.0..=1.0).contains(h))
    }
}

fn landform(look: Look, seed: u32, x: f32, z: f32) -> f32 {
    let warp = noise(x * 3.1 + 8.0, z * 2.5, seed) - 0.5;
    let detail = fractal(x * 8.0, z * 8.0, seed, 5);
    let ribs = 1.0 - (2.0 * fractal(x * 9.0 + warp * 2.0, z * 6.0, seed ^ 725, 4) - 1.0).abs();
    let peak = |px: f32, pz: f32, sx: f32, sz: f32| {
        let d = ((x - px + warp * 0.15) / sx).hypot((z - pz) / sz);
        (1.0 - d).max(0.0)
    };
    let edge = 1.0 - smooth(0.87, 1.0, x.abs().max(z.abs()));
    let height = match look {
        Look::Alpine => {
            let left = peak(-0.54, -0.37, 0.70, 0.94);
            let right = peak(0.62, -0.57, 0.79, 0.94);
            let saddle = peak(0.02, -0.85, 0.60, 0.79);
            let massif = (left * 0.90).max(right).max(saddle * 0.66);
            let shore =
                (peak(-0.78, 0.61, 0.62, 0.52) * 0.27).max(peak(0.82, 0.22, 0.60, 0.38) * 0.29);
            let base = 0.039 + detail * 0.075;
            base + shore * (0.5 + detail)
                + massif.powf(0.78) * (0.55 + ribs * 0.34)
                + massif.sqrt() * (detail - 0.49) * 0.48
        }
        Look::Canyon => {
            let curve = 0.26 * (z * 4.2 + 0.6).sin() - 0.08 * (z * 9.0).cos();
            let bank = (x - curve).abs() + warp * 0.13;
            let escarpment = smooth(0.13, 0.48, bank);
            let mesa = smooth(0.58, 0.76, bank) * (0.75 + detail * 0.35);
            let butte = peak(0.17, -0.62, 0.36, 0.51) * 0.36;
            (0.042
                + escarpment * (0.32 + 0.41 * mesa)
                + butte
                + (detail - 0.50) * (0.12 + escarpment * 0.40)
                + escarpment * 0.045 * (z * 55.0 + warp * 7.0).sin())
                * (0.9 + 0.09 * z)
        }
        Look::Coast => {
            let volcano = peak(0.23, -0.38, 0.91, 1.00);
            let ridge = peak(-0.48, -0.42, 0.56, 0.71);
            let island = peak(-0.53, 0.54, 0.34, 0.35);
            let main = volcano.powf(1.18) * 0.80 + ridge.powf(0.95) * 0.43;
            let caldera = (1.0 - ((x - 0.23).hypot(z + 0.38) / 0.12)).max(0.0);
            0.026 + main * (0.68 + 0.37 * ribs) + island * 0.33 - 0.14 * caldera
                + (detail - 0.43) * (0.068 + main * 0.39)
        }
    };
    (height * edge).clamp(0.0, 1.0)
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Camera {
    pub heading: f32,
    pub elevation: f32,
    pub distance: f32,
    pub target_height: f32,
    pub target_x: f32,
    pub target_z: f32,
    pub field_of_view: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lens: Option<CameraLens>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct CameraLens {
    pub roll: f32,
    pub shift: [f32; 2],
    pub scale: [f32; 2],
}

impl CameraLens {
    fn valid(&self) -> bool {
        (-180.0..=180.0).contains(&self.roll)
            && self.shift.iter().all(|v| (-8.0..=8.0).contains(v))
            && self.scale.iter().all(|v| (0.005..=100.0).contains(v))
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Atmosphere {
    pub sun_heading: f32,
    pub sun_elevation: f32,
    pub haze: f32,
    pub clouds: f32,
    pub warmth: f32,
    pub zenith: [f32; 3],
    pub horizon: [f32; 3],
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Water {
    pub enabled: bool,
    pub level: f32,
    pub ripples: f32,
    pub mirror: f32,
    pub material: Material,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Scene {
    version: u32,
    pub look: Look,
    pub seed: u32,
    pub terrain: Terrain,
    pub terrain_visible: bool,
    pub terrain_color: Option<Material>,
    pub objects: Vec<Object>,
    pub relief: f32,
    pub snowline: f32,
    pub camera: Camera,
    pub atmosphere: Atmosphere,
    pub water: Water,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_size: Option<[u32; 2]>,
}

impl Scene {
    pub fn preset(look: Look, seed: u32) -> Self {
        let (heading, elevation, distance, target_height, sun_heading, sun_elevation, warmth) =
            match look {
                Look::Alpine => (-5.0, 6.4, 1.55, 0.13, -55.0, 22.0, 0.30),
                Look::Canyon => (-12.0, 25.0, 1.85, 0.23, 98.0, 14.0, 0.87),
                Look::Coast => (18.0, 5.9, 1.52, 0.11, -62.0, 9.0, 0.62),
            };
        Self {
            version: 1,
            look,
            seed,
            terrain: Terrain::generate(look, seed),
            terrain_visible: true,
            terrain_color: None,
            objects: Vec::new(),
            relief: match look {
                Look::Alpine => 0.91,
                Look::Canyon => 0.63,
                Look::Coast => 0.76,
            },
            snowline: match look {
                Look::Alpine => 0.65,
                Look::Canyon => 1.0,
                Look::Coast => 0.98,
            },
            camera: Camera {
                heading,
                elevation,
                distance,
                target_height,
                target_x: 0.0,
                target_z: -0.18,
                field_of_view: 49.0,
                lens: None,
            },
            atmosphere: Atmosphere {
                sun_heading,
                sun_elevation,
                warmth,
                zenith: [0.22, 0.43, 0.72],
                horizon: [0.71, 0.85, 0.95],
                haze: if look == Look::Canyon { 0.40 } else { 0.23 },
                clouds: if look == Look::Alpine { 0.65 } else { 0.50 },
            },
            output_size: None,
            water: Water {
                enabled: true,
                level: match look {
                    Look::Alpine => 0.10,
                    Look::Canyon => 0.055,
                    Look::Coast => 0.093,
                },
                ripples: if look == Look::Coast { 0.46 } else { 0.24 },
                mirror: if look == Look::Alpine { 0.93 } else { 0.81 },
                material: Material::solid([0.10, 0.27, 0.36]),
            },
        }
    }

    pub fn can_add(&self, object: &Object) -> bool {
        let (mesh, image, heights) = Self::asset_units(self.objects.iter().chain([object]));
        self.objects.len() < MAX_OBJECTS
            && mesh <= 450_000
            && image <= 2_097_152
            && heights <= 2_000_000
    }

    fn asset_units<'a>(objects: impl IntoIterator<Item = &'a Object>) -> (usize, usize, usize) {
        objects.into_iter().fold((0, 0, 0), |(m, b, h), object| {
            (
                m + object.mesh.as_ref().map_or(0, |data| data.units()),
                b + object.bitmap.as_ref().map_or(0, |data| data.units()),
                h + object
                    .heightmap
                    .as_ref()
                    .map_or(0, |data| data.heights.len()),
            )
        })
    }

    pub fn camera_target(&self) -> Vec3 {
        Vec3(
            self.camera.target_x,
            self.camera.target_height,
            self.camera.target_z,
        )
    }

    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        if file.metadata()?.len() > MAX_PROJECT {
            return Err("project exceeds the 64 MiB limit".into());
        }
        let scene: Self = serde_json::from_reader(BufReader::new(file))?;
        if !scene.valid() {
            return Err("unsupported project version or out-of-range scene values".into());
        }
        Ok(scene)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        atomic_write(path, |file| {
            let mut writer = io::BufWriter::new(file);
            serde_json::to_writer(&mut writer, self)?;
            writer.flush()?;
            Ok(())
        })
    }

    pub fn valid(&self) -> bool {
        let camera = &self.camera;
        let sky = &self.atmosphere;
        let (mesh, image, heights) = Self::asset_units(&self.objects);
        self.version == 1
            && mesh <= 450_000
            && image <= 2_097_152
            && heights <= 2_000_000
            && self
                .output_size
                .is_none_or(|size| size.into_iter().all(|v| (16..=4096).contains(&v)))
            && camera.lens.as_ref().is_none_or(CameraLens::valid)
            && self.terrain.validate()
            && self.relief.is_finite()
            && self.objects.len() <= MAX_OBJECTS
            && self.objects.iter().all(Object::valid)
            && self.terrain_color.as_ref().is_none_or(Material::valid)
            && self.water.material.valid()
            && sky
                .zenith
                .iter()
                .chain(&sky.horizon)
                .all(|v| (0.0..=1.0).contains(v))
            && (0.05..=2.0).contains(&self.relief)
            && [
                self.snowline,
                sky.haze,
                sky.clouds,
                sky.warmth,
                self.water.level,
                self.water.ripples,
                self.water.mirror,
            ]
            .iter()
            .all(|v| (0.0..=1.0).contains(v))
            && (-180.0..=180.0).contains(&camera.heading)
            && (-180.0..=180.0).contains(&sky.sun_heading)
            && (-89.99..=89.99).contains(&camera.elevation)
            && (1.0..=89.0).contains(&sky.sun_elevation)
            && (0.05..=32.0).contains(&camera.distance)
            && [camera.target_x, camera.target_height, camera.target_z]
                .into_iter()
                .all(|v| (-128.0..=128.0).contains(&v))
            && (1.0..=179.0).contains(&camera.field_of_view)
    }
}

pub fn atomic_write(path: &Path, write: impl FnOnce(&mut File) -> Result<()>) -> Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().ok_or("destination has no parent directory")?;
    let (temporary, mut file) = loop {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!(".terrain-studio-{}-{id}.tmp", std::process::id()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => break (temporary, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    };
    let result = write(&mut file).and_then(|()| {
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        Ok(())
    });
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Mirage,
    Checker,
    Mercury,
    Alpine,
    Canyon,
    Coast,
}

impl Preset {
    pub const ALL: [Self; 6] = [
        Self::Mirage,
        Self::Checker,
        Self::Mercury,
        Self::Alpine,
        Self::Canyon,
        Self::Coast,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Mirage => "Neon Mirage",
            Self::Checker => "Checker Study",
            Self::Mercury => "Mercury Plain",
            Self::Alpine => Look::Alpine.name(),
            Self::Canyon => Look::Canyon.name(),
            Self::Coast => Look::Coast.name(),
        }
    }

    pub fn from_slug(slug: &str) -> Option<Self> {
        match slug {
            "mirage" => Some(Self::Mirage),
            "checker" => Some(Self::Checker),
            "mercury" => Some(Self::Mercury),
            _ => Look::from_slug(slug).map(|look| match look {
                Look::Alpine => Self::Alpine,
                Look::Canyon => Self::Canyon,
                Look::Coast => Self::Coast,
            }),
        }
    }

    pub fn scene(self, seed: u32) -> Scene {
        match self {
            Self::Mirage => mirage(seed),
            Self::Checker => checker(seed),
            Self::Mercury => mercury(seed),
            Self::Alpine => Scene::preset(Look::Alpine, seed),
            Self::Canyon => Scene::preset(Look::Canyon, seed),
            Self::Coast => Scene::preset(Look::Coast, seed),
        }
    }
}

fn mirage(seed: u32) -> Scene {
    let mut scene = Scene::preset(Look::Alpine, seed);
    scene.relief = 0.38;
    scene.terrain_visible = true;
    scene.terrain_color = Some(Material::new(
        Pattern::Marble,
        [1.0, 0.23, 0.025],
        [0.8, 0.025, 0.16],
    ));
    scene.water.level = 0.089;
    scene.water.ripples = 0.30;
    scene.water.mirror = 0.92;
    scene.water.material = Material::solid([0.16, 0.12, 0.76]);
    (
        scene.camera.heading,
        scene.camera.elevation,
        scene.camera.distance,
    ) = (2.0, 10.5, 1.42);
    (
        scene.camera.target_height,
        scene.camera.target_z,
        scene.camera.field_of_view,
    ) = (0.22, -0.18, 53.0);
    let sky = &mut scene.atmosphere;
    sky.zenith = [0.26, 0.16, 1.0];
    sky.horizon = [0.44, 0.43, 0.98];
    (
        sky.clouds,
        sky.haze,
        sky.warmth,
        sky.sun_heading,
        sky.sun_elevation,
    ) = (0.06, 0.08, 0.48, 54.0, 31.0);
    let mut crystal = Material::new(Pattern::Marble, [0.99, 0.19, 0.86], [0.34, 0.15, 0.88]);
    crystal.reflectivity = 0.38;
    crystal.roughness = 0.23;
    crystal.metallic = 0.51;
    crystal.density = 2.2;
    let mut check = Material::new(Pattern::Checker, [0.86, 0.63, 0.28], [0.19, 0.055, 0.25]);
    check.density = 5.0;
    let mut ring = Material::solid([0.95, 0.32, 0.54]);
    ring.metallic = 0.84;
    ring.reflectivity = 0.71;
    scene.objects = vec![
        Object::new(Shape::Pyramid)
            .named("Tall crystal")
            .at([-0.18, 0.35, -0.38], [0.32, 0.36, 0.24])
            .rotated([0.0, -27.0, 0.0])
            .painted(crystal.clone()),
        Object::new(Shape::Pyramid)
            .named("Near crystal")
            .at([0.32, 0.205, 0.62], [0.135, 0.19, 0.13])
            .rotated([0.0, 31.0, 0.0])
            .painted(crystal.clone()),
        Object::new(Shape::Pyramid)
            .named("West crystal")
            .at([-0.37, 0.133, 0.64], [0.105, 0.11, 0.11])
            .rotated([0.0, -12.0, 0.0])
            .painted(crystal),
        Object::new(Shape::Sphere)
            .named("Checker moon")
            .at([0.53, 0.72, -0.68], [0.125; 3])
            .rotated([21.0, 18.0, 10.0])
            .painted(check),
        Object::new(Shape::Torus)
            .named("Chrome arch")
            .at([-0.68, 0.32, -0.24], [0.225; 3])
            .rotated([74.0, 28.0, 14.0])
            .painted(ring),
    ];
    scene
}

fn checker(seed: u32) -> Scene {
    let mut scene = Scene::preset(Look::Alpine, seed);
    scene.terrain_visible = false;
    scene.water.level = 0.037;
    scene.water.ripples = 0.48;
    scene.water.material = Material::solid([0.12, 0.57, 0.68]);
    (
        scene.camera.heading,
        scene.camera.elevation,
        scene.camera.distance,
    ) = (13.0, 16.0, 1.72);
    (
        scene.camera.target_height,
        scene.camera.target_z,
        scene.camera.field_of_view,
    ) = (0.33, 0.0, 52.0);
    let sky = &mut scene.atmosphere;
    sky.zenith = [0.14, 0.30, 0.78];
    sky.horizon = [0.52, 0.77, 0.98];
    (sky.clouds, sky.haze, sky.sun_heading, sky.sun_elevation) = (0.44, 0.11, 51.0, 31.0);
    let mut tile = Material::new(Pattern::Checker, [0.93, 0.95, 0.99], [0.045, 0.055, 0.12]);
    tile.reflectivity = 0.53;
    tile.density = 5.0;
    let mut red = Material::solid([0.94, 0.21, 0.035]);
    red.roughness = 0.13;
    red.metallic = 0.22;
    let mut blue = Material::solid([0.055, 0.25, 0.86]);
    blue.metallic = 0.70;
    blue.reflectivity = 0.40;
    let plaster = Material::solid([0.63, 0.66, 0.72]);
    scene.objects = vec![
        Object::new(Shape::Box)
            .named("Checker platform")
            .at([0.0, 0.046, 0.0], [0.70, 0.023, 0.61])
            .painted(tile),
        Object::new(Shape::Box)
            .named("Tall tower")
            .at([-0.13, 0.40, -0.25], [0.10, 0.34, 0.097])
            .rotated([0.0, 14.0, 0.0])
            .painted(plaster.clone()),
        Object::new(Shape::Box)
            .named("Middle tower")
            .at([0.12, 0.306, -0.20], [0.115, 0.239, 0.105])
            .rotated([0.0, 14.0, 0.0])
            .painted(plaster.clone()),
        Object::new(Shape::Box)
            .named("Short tower")
            .at([-0.34, 0.192, -0.03], [0.084, 0.125, 0.094])
            .painted(plaster),
        Object::new(Shape::Sphere)
            .named("Copper sphere")
            .at([-0.44, 0.64, 0.02], [0.16; 3])
            .painted(red),
        Object::new(Shape::Cone)
            .named("Blue cone")
            .at([0.43, 0.62, -0.04], [0.165, 0.208, 0.165])
            .rotated([18.0, 0.0, 22.0])
            .painted(blue),
        Object::new(Shape::Box)
            .named("Lime cube")
            .at([0.05, 0.215, 0.29], [0.13; 3])
            .rotated([18.0, -19.0, 22.0])
            .painted(Material::solid([0.33, 0.91, 0.14])),
    ];
    scene
}

fn mercury(seed: u32) -> Scene {
    let mut scene = Scene::preset(Look::Alpine, seed);
    scene.terrain_visible = false;
    scene.water.level = 0.03;
    scene.water.ripples = 0.08;
    scene.water.mirror = 0.66;
    scene.water.material = Material::new(Pattern::Marble, [0.44, 0.46, 0.52], [0.11, 0.096, 0.16]);
    scene.water.material.density = 6.0;
    (
        scene.camera.heading,
        scene.camera.elevation,
        scene.camera.distance,
    ) = (-4.0, 19.0, 1.46);
    (
        scene.camera.target_height,
        scene.camera.target_z,
        scene.camera.field_of_view,
    ) = (0.29, -0.04, 54.0);
    let sky = &mut scene.atmosphere;
    sky.zenith = [0.24, 0.32, 0.57];
    sky.horizon = [0.68, 0.80, 0.97];
    (sky.clouds, sky.haze, sky.warmth) = (0.74, 0.22, 0.22);
    let metal = |color, accent| {
        let mut m = Material::new(Pattern::Marble, color, accent);
        m.metallic = 0.73;
        m.reflectivity = 0.68;
        m
    };
    let mut lava = Material::new(Pattern::Lava, [0.22, 0.025, 0.018], [1.0, 0.27, 0.028]);
    lava.density = 5.5;
    scene.objects = vec![
        Object::new(Shape::Torus)
            .named("Mercury loop")
            .at([0.07, 0.32, 0.28], [0.27; 3])
            .rotated([68.0, 31.0, -20.0])
            .painted(metal([0.85, 0.90, 0.98], [0.35, 0.46, 0.72])),
        Object::new(Shape::Pyramid)
            .named("Basalt prism")
            .at([-0.46, 0.18, 0.2], [0.20, 0.18, 0.20])
            .rotated([0.0, 23.0, 0.0])
            .painted(metal([0.75, 0.17, 0.21], [0.21, 0.29, 0.61])),
        Object::new(Shape::Box)
            .named("Patina cube")
            .at([-0.25, 0.58, -0.48], [0.145; 3])
            .rotated([2.0, -18.0, 0.0])
            .painted(metal([0.075, 0.52, 0.55], [0.025, 0.11, 0.15])),
        Object::new(Shape::Sphere)
            .named("Lava sphere")
            .at([0.49, 0.53, -0.26], [0.125; 3])
            .painted(lava),
    ];
    scene
}
