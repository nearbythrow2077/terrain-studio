use crate::assets::{Bitmap, Mesh};
use crate::math::Vec3;
use crate::scene::Terrain;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Shape {
    Sphere,
    Box,
    Pyramid,
    Cone,
    Cylinder,
    Torus,
    Mesh,
    Image,
    Terrain,
}

impl Shape {
    pub const ALL: [Self; 6] = [
        Self::Sphere,
        Self::Box,
        Self::Pyramid,
        Self::Cone,
        Self::Cylinder,
        Self::Torus,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Sphere => "Sphere",
            Self::Box => "Box",
            Self::Pyramid => "Pyramid",
            Self::Cone => "Cone",
            Self::Cylinder => "Cylinder",
            Self::Torus => "Torus",
            Self::Mesh => "Mesh",
            Self::Image => "Image panel",
            Self::Terrain => "Terrain",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pattern {
    Solid,
    Checker,
    Marble,
    Lava,
    Ripple,
}

impl Pattern {
    pub const ALL: [Self; 5] = [
        Self::Solid,
        Self::Checker,
        Self::Marble,
        Self::Lava,
        Self::Ripple,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Solid => "Solid",
            Self::Checker => "Checker",
            Self::Marble => "Marble",
            Self::Lava => "Lava",
            Self::Ripple => "Ripples",
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Material {
    pub pattern: Pattern,
    pub color: [f32; 3],
    pub accent: [f32; 3],
    pub density: f32,
    pub reflectivity: f32,
    pub roughness: f32,
    pub metallic: f32,
}

impl Material {
    pub fn new(pattern: Pattern, color: [f32; 3], accent: [f32; 3]) -> Self {
        Self {
            pattern,
            color,
            accent,
            density: 4.0,
            reflectivity: 0.20,
            roughness: 0.19,
            metallic: 0.0,
        }
    }

    pub fn solid(color: [f32; 3]) -> Self {
        Self::new(Pattern::Solid, color, color)
    }

    pub fn valid(&self) -> bool {
        self.color
            .iter()
            .chain(&self.accent)
            .chain([&self.reflectivity, &self.roughness, &self.metallic])
            .all(|v| (0.0..=1.0).contains(v))
            && (0.2..=64.0).contains(&self.density)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Object {
    pub name: String,
    pub shape: Shape,
    pub position: [f32; 3],
    pub scale: [f32; 3],
    pub rotation: [f32; 3],
    pub material: Material,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mesh: Option<Arc<Mesh>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bitmap: Option<Arc<Bitmap>>,
    #[serde(default)]
    pub smooth_mesh: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heightmap: Option<Arc<Terrain>>,
    #[serde(default)]
    pub natural_terrain: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_basis: Option<[[f32; 3]; 3]>,
}

impl Object {
    pub fn new(shape: Shape) -> Self {
        Self {
            name: shape.name().into(),
            shape,
            position: [0.0, 0.33, 0.0],
            scale: [0.18; 3],
            rotation: [0.0; 3],
            material: Material::solid([0.78, 0.26, 0.15]),
            mesh: None,
            bitmap: None,
            smooth_mesh: false,
            heightmap: None,
            natural_terrain: false,
            model_basis: None,
        }
    }

    pub fn at(mut self, position: [f32; 3], scale: [f32; 3]) -> Self {
        self.position = position;
        self.scale = scale;
        self
    }

    pub fn rotated(mut self, rotation: [f32; 3]) -> Self {
        self.rotation = rotation;
        self
    }

    pub fn painted(mut self, material: Material) -> Self {
        self.material = material;
        self
    }

    pub fn named(mut self, name: &str) -> Self {
        self.name = name.into();
        self
    }

    pub fn mesh(name: &str, mesh: Mesh) -> Self {
        let mut object = Self::new(Shape::Mesh).named(name);
        object.mesh = Some(Arc::new(mesh));
        object.smooth_mesh = true;
        object.material = Material::solid([0.77, 0.77, 0.82]);
        object
    }

    pub fn picture(name: &str, bitmap: Bitmap) -> Self {
        let ratio = bitmap.width as f32 / bitmap.height as f32;
        let scale = [
            (0.26 * ratio).clamp(0.01, 0.26),
            (0.26 / ratio).clamp(0.01, 0.26),
            0.005,
        ];
        let mut object = Self::new(Shape::Image)
            .named(name)
            .at([0.0, 0.38, 0.0], scale);
        object.bitmap = Some(Arc::new(bitmap));
        object.material.roughness = 0.72;
        object.material.reflectivity = 0.05;
        object
    }

    pub fn valid(&self) -> bool {
        ((self.shape == Shape::Mesh) == self.mesh.is_some())
            && ((self.shape == Shape::Image) == self.bitmap.is_some())
            && ((self.shape == Shape::Terrain) == self.heightmap.is_some())
            && self.heightmap.as_ref().is_none_or(|map| map.validate())
            && self.model_basis.is_none_or(|basis| {
                let [x, y, z] = basis.map(Vec3::from_array);
                basis.iter().flatten().all(|v| (-128.0..=128.0).contains(v))
                    && x.dot(y.cross(z)).abs() > 1e-12
            })
            && self.mesh.as_ref().is_none_or(|mesh| mesh.valid())
            && self.bitmap.as_ref().is_none_or(|bitmap| bitmap.valid())
            && self.name.chars().count() <= 80
            && self.position.iter().all(|v| (-128.0..=128.0).contains(v))
            && self.scale.iter().all(|v| (0.005..=4.0).contains(v))
            && self.rotation.iter().all(|v| (-180.0..=180.0).contains(v))
            && self.material.valid()
    }
}

pub fn linear(color: [f32; 3]) -> Vec3 {
    let channel = |c: f32| {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    Vec3(channel(color[0]), channel(color[1]), channel(color[2]))
}
