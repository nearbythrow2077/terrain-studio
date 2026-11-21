use crate::math::Vec3;
use crate::objects::{Material, Object, Shape};
use crate::scene::{CameraLens, Look, Result, Scene, Terrain};
use image::ImageBuffer;
use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;

const HEADER_END: u64 = 0x290;
const STREAM_HEADER: usize = 12;
const HEIGHT_HEADER: usize = 0x350;
const TABLE_LIMIT: usize = 1024 * 1024;

pub struct LegacyScene {
    file: File,
    length: u64,
    maps: Vec<LegacyTerrain>,
    resources: Vec<Resource>,
}

#[derive(Clone, Copy)]
struct Resource {
    offset: u64,
    bytes: usize,
}

#[derive(Clone, Copy)]
pub struct LegacyTerrain {
    pub index: usize,
    pub side: u32,
    offset: u64,
    bytes: usize,
}

impl LegacyScene {
    pub fn open(path: &Path) -> Result<Self> {
        let mut file = File::open(path)?;
        let length = file.metadata()?.len();
        let mut header = [0; HEADER_END as usize];
        file.read_exact(&mut header)?;
        if !header.starts_with(b"CCmF - ") || header[0x248] != 1 {
            return Err("unsupported Bryce 2 composite file header".into());
        }
        let bytes = read_u32(&header[0x24c..]) as usize;
        let offset = u64::from(read_u32(&header[0x250..]));
        if bytes == 0 || bytes > TABLE_LIMIT {
            return Err("invalid Bryce 2 resource table length".into());
        }
        let table = read_chain(&mut file, length, offset, bytes, bytes)?;
        let (entries, suffix) = table.as_chunks::<STREAM_HEADER>();
        if !suffix.is_empty() {
            return Err("invalid Bryce 2 resource table length".into());
        }
        let resources: Vec<_> = entries
            .iter()
            .map(|entry| Resource {
                bytes: read_u32(&entry[4..]) as usize,
                offset: if entry[0] == 1 {
                    u64::from(read_u32(&entry[8..]))
                } else {
                    0
                },
            })
            .collect();
        let mut maps = Vec::new();
        for (index, resource) in resources.iter().enumerate() {
            let Some(side) = square_resolution(resource.bytes) else {
                continue;
            };
            if resource.offset == 0 {
                continue;
            }
            let prefix = read_chain(
                &mut file,
                length,
                resource.offset,
                resource.bytes,
                HEIGHT_HEADER,
            )?;
            if height_header_matches(&prefix, side) {
                maps.push(LegacyTerrain {
                    index,
                    side,
                    offset: resource.offset,
                    bytes: resource.bytes,
                });
            }
        }
        let scene = Self {
            file,
            length,
            maps,
            resources,
        };
        Ok(scene)
    }

    fn resource(&mut self, index: usize, cap: usize) -> Result<Option<Vec<u8>>> {
        let resource = self
            .resources
            .get(index)
            .ok_or("Bryce 2 resource index is out of range")?;
        if resource.offset == 0 {
            return Ok(None);
        }
        if resource.bytes == 0 || resource.bytes > cap {
            return Err("unexpected Bryce 2 scene resource size".into());
        }
        read_chain(
            &mut self.file,
            self.length,
            resource.offset,
            resource.bytes,
            resource.bytes,
        )
        .map(Some)
    }

    fn class_members(&mut self) -> Result<Vec<ClassMembers>> {
        let entries = self
            .resource(16, TABLE_LIMIT)?
            .ok_or("Bryce 2 class directory is missing")?;
        let header = self
            .resource(17, 32)?
            .ok_or("Bryce 2 class directory header is missing")?;
        let rows = object_rows(&header, &entries, 32)?;
        let mut classes = Vec::new();
        for row in rows {
            let name = resource_name(&row[..16]).ok_or("invalid Bryce 2 class name")?;
            let index = read_u32(&row[16..]);
            let meta_index = read_u32(&row[20..]);
            let members = self
                .resource(index as usize, TABLE_LIMIT)?
                .ok_or("missing Bryce 2 class member directory")?;
            let meta = self
                .resource(meta_index as usize, 32)?
                .ok_or("missing Bryce 2 class member header")?;
            let objects = object_rows(&meta, &members, 60)?;
            classes.push(ClassMembers {
                name: name.to_owned(),
                resources: objects
                    .map(|row| [read_u32(&row[4..]) as usize, read_u32(&row[8..]) as usize])
                    .collect(),
            });
        }
        Ok(classes)
    }

    pub fn open_project(path: &Path) -> Result<ImportedScene> {
        let mut file = Self::open(path)?;
        let classes = file.class_members()?;
        let mut camera = None;
        let mut planes = Vec::new();
        let mut output_size = None;
        let mut placement = Vec::new();
        let mut skipped = 0;
        for class in classes {
            match class.name.as_str() {
                "C3dBaseCamera" => {
                    for [index, _] in class.resources {
                        let Some(data) = file.resource(index, 884)? else {
                            continue;
                        };
                        if data.starts_with(b"perspective\0") {
                            camera = Some(decode_camera(&data)?);
                        }
                    }
                }
                "C3dHeightField" => {
                    for [index, _] in class.resources {
                        if file.maps.iter().any(|map| map.index == index) {
                            let Some(header) = file.resource_prefix(index, HEIGHT_HEADER)? else {
                                continue;
                            };
                            let transform = ModelTransform::terrain(&header)?;
                            placement.push((index, Shape::Terrain, transform));
                        } else {
                            skipped += 1;
                        }
                    }
                }
                "C3dSphere" | "C3dCube" => {
                    let shape = if class.name == "C3dSphere" {
                        Shape::Sphere
                    } else {
                        Shape::Box
                    };
                    for [index, _] in class.resources {
                        let Some(data) = file.resource(index, 749)? else {
                            skipped += 1;
                            continue;
                        };
                        let mut transform = ModelTransform::read(&data)?;
                        // Bryce's sphere is unit radius. Its cube spans -0.5 to +0.5.
                        if shape == Shape::Box {
                            transform.basis = transform.basis.map(|axis| axis * 0.5);
                        }
                        placement.push((index, shape, transform));
                    }
                }
                "C3dPlane" => {
                    for [index, _] in class.resources {
                        let Some(data) = file.resource(index, 749)? else {
                            skipped += 1;
                            continue;
                        };
                        let transform = ModelTransform::read(&data)?;
                        let length = transform.basis[1].length();
                        if length > 0.0 && transform.basis[1].1.abs() / length > 0.9999 {
                            planes.push(transform.position.1);
                        } else {
                            skipped += 1;
                        }
                    }
                }
                "brycedocset" => {
                    if let Some(&[_, index]) = class.resources.first() {
                        if let Some(data) =
                            file.resource(index, 416)?.filter(|data| data.len() == 416)
                        {
                            let size = [read_u32(&data[8..]), read_u32(&data[12..])];
                            if size.into_iter().all(|v| (16..=4096).contains(&v)) {
                                output_size = Some(size);
                            }
                        }
                    }
                }
                "C3dBaseSky" | "C3dSunLight" | "C3dTexture" | "C3dBaseMaterial" | "C3dCImage"
                | "CPictTexture" => {}
                _ => skipped += class.resources.len(),
            }
        }
        let camera = camera.ok_or("Bryce 2 perspective camera is missing")?;
        let mut scene = Scene::preset(Look::Alpine, 42);
        scene.terrain_visible = false;
        scene.terrain = Terrain {
            width: 2,
            depth: 2,
            heights: vec![0.0; 4],
        };
        scene.output_size = output_size;
        let water = planes
            .iter()
            .copied()
            .filter(|&height| height <= camera.eye.1)
            .max_by(f32::total_cmp);
        // A higher horizontal plane may belong to an atmospheric layer.
        skipped += planes.len() - usize::from(water.is_some());
        let scale = placement
            .iter()
            .map(|(_, _, form)| {
                (form.position - camera.eye).length()
                    + form.basis.into_iter().map(Vec3::length).sum::<f32>()
            })
            .fold(0.0_f32, f32::max)
            .max(water.map_or(0.0, |plane| (camera.eye.1 - plane).abs()))
            .max(0.25)
            / 24.0;
        let look = (camera.target - camera.eye).dot(camera.forward) / scale;
        let orbit = if look.is_finite() && look > 0.05 {
            look.clamp(0.05, 24.0)
        } else {
            1.5
        };
        let aim = camera.eye + camera.forward * (orbit * scale);
        let origin = Vec3(aim.0, water.unwrap_or(aim.1), aim.2);
        let target = (aim - origin) / scale;
        scene.camera.heading = (-camera.forward.0).atan2(-camera.forward.2).to_degrees();
        scene.camera.elevation = (-camera.forward.1)
            .clamp(-1.0, 1.0)
            .asin()
            .to_degrees()
            .clamp(-89.99, 89.99);
        scene.camera.distance = orbit;
        scene.camera.target_x = target.0;
        scene.camera.target_height = target.1;
        scene.camera.target_z = target.2;
        scene.camera.field_of_view = camera.fov;
        scene.camera.lens = Some(camera.lens);
        scene.water.enabled = water.is_some();
        scene.water.level = 0.0;
        scene.water.ripples = 0.15;
        scene.water.mirror = 0.86;
        let mut terrains = 0;
        let mut primitives = 0;
        for (index, shape, form) in placement {
            let position = ((form.position - origin) / scale).to_array();
            let basis = form.basis.map(|axis| (axis / scale).to_array());
            let mut object = Object::new(shape)
                .named(&format!("{} {index}", shape.name()))
                .at(position, [1.0; 3])
                .painted(Material::solid([0.48, 0.47, 0.45]));
            object.material.reflectivity = 0.0;
            object.material.roughness = 0.85;
            if shape == Shape::Terrain {
                object.heightmap = Some(Arc::new(file.terrain(Some(index))?));
                object.natural_terrain = true;
            }
            object.model_basis = Some(basis);
            if scene.can_add(&object) && object.valid() {
                scene.objects.push(object);
                terrains += usize::from(shape == Shape::Terrain);
                primitives += usize::from(shape != Shape::Terrain);
            } else {
                skipped += 1;
            }
        }
        if !scene.valid() {
            return Err(
                "Bryce 2 scene uses camera or placement values outside the supported range".into(),
            );
        }
        Ok(ImportedScene {
            scene,
            terrains,
            primitives,
            skipped,
        })
    }

    fn resource_prefix(&mut self, index: usize, prefix: usize) -> Result<Option<Vec<u8>>> {
        let resource = self
            .resources
            .get(index)
            .ok_or("Bryce 2 resource index is out of range")?;
        if resource.offset == 0 {
            return Ok(None);
        }
        if resource.bytes < prefix {
            return Err("truncated Bryce 2 scene resource".into());
        }
        read_chain(
            &mut self.file,
            self.length,
            resource.offset,
            resource.bytes,
            prefix,
        )
        .map(Some)
    }

    pub fn maps(&self) -> &[LegacyTerrain] {
        &self.maps
    }

    pub fn terrain(&mut self, index: Option<usize>) -> Result<Terrain> {
        let map = match index {
            Some(index) => self
                .maps
                .iter()
                .find(|map| map.index == index)
                .ok_or_else(|| format!("Bryce 2 resource {index} is not a supported heightmap"))?,
            None => self
                .maps
                .iter()
                .min_by_key(|map| (std::cmp::Reverse(map.side), map.index))
                .ok_or("no supported heightmaps found in the Bryce 2 scene")?,
        };
        let payload = read_chain(
            &mut self.file,
            self.length,
            map.offset,
            map.bytes,
            map.bytes,
        )?;
        if !height_header_matches(&payload[..HEIGHT_HEADER], map.side) {
            return Err("Bryce 2 heightmap changed after opening the scene".into());
        }
        let samples = payload[HEIGHT_HEADER..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|word| u16::from_be_bytes(*word))
            .collect();
        let image = ImageBuffer::from_raw(map.side, map.side, samples)
            .ok_or("invalid Bryce 2 heightmap dimensions")?;
        Ok(Terrain::from_luma16(image))
    }
}

pub struct ImportedScene {
    pub scene: Scene,
    pub terrains: usize,
    pub primitives: usize,
    pub skipped: usize,
}

impl ImportedScene {
    pub fn summary(&self) -> String {
        format!("{} terrains and {} primitives restored · {} other objects or planes omitted · Approximate materials and sky",
            self.terrains, self.primitives, self.skipped)
    }
}

struct ClassMembers {
    name: String,
    resources: Vec<[usize; 2]>,
}

fn object_rows<'a>(
    meta: &[u8],
    payload: &'a [u8],
    stride: usize,
) -> Result<impl Iterator<Item = &'a [u8]>> {
    if meta.len() != 32 || read_u32(meta) as usize != stride {
        return Err("unexpected Bryce 2 class directory record width".into());
    }
    let count = read_u32(&meta[12..]) as usize;
    if count > payload.len() / stride {
        return Err("Bryce 2 class directory record count exceeds its resource".into());
    }
    Ok(payload.chunks_exact(stride).take(count))
}

fn resource_name(bytes: &[u8]) -> Option<&str> {
    let end = bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len());
    std::str::from_utf8(&bytes[..end])
        .ok()
        .filter(|s| !s.is_empty())
}

struct ModelTransform {
    position: Vec3,
    basis: [Vec3; 3],
}

impl ModelTransform {
    fn read(data: &[u8]) -> Result<Self> {
        if data.len() < 0x2dc {
            return Err("short Bryce 2 object transform".into());
        }
        let position = le_vec(data, 0x2c4);
        let basis = [
            le_vec(data, 0x27c),
            le_vec(data, 0x294),
            le_vec(data, 0x2ac),
        ];
        if !position
            .to_array()
            .into_iter()
            .chain(basis.into_iter().flat_map(Vec3::to_array))
            .all(|v| v.is_finite() && v.abs() <= 1e8)
        {
            return Err("invalid Bryce 2 object transform".into());
        }
        Ok(Self { position, basis })
    }

    fn terrain(data: &[u8]) -> Result<Self> {
        let mut transform = Self::read(data)?;
        let [width, height, depth] = [
            be_f64(data, 0x308),
            be_f64(data, 0x310),
            be_f64(data, 0x318),
        ];
        if ![width, height, depth]
            .into_iter()
            .all(|v| (1.0..=1024.0).contains(&v))
        {
            return Err("invalid Bryce 2 terrain extents".into());
        }
        transform.basis[0] = transform.basis[0] * (width as f32 * 0.5);
        transform.basis[1] = transform.basis[1] * height as f32;
        transform.basis[2] = transform.basis[2] * (depth as f32 * 0.5);
        transform.position = transform.position - transform.basis[1] * 0.5;
        Ok(transform)
    }
}

struct LegacyCamera {
    eye: Vec3,
    forward: Vec3,
    target: Vec3,
    fov: f32,
    lens: CameraLens,
}

fn decode_camera(data: &[u8]) -> Result<LegacyCamera> {
    if data.len() != 884 {
        return Err("unexpected Bryce 2 perspective camera record size".into());
    }
    let eye = ModelTransform::read(data)?.position;
    let forward = be_vec(data, 0x2ec);
    let up = be_vec(data, 0x304);
    if (forward.length() - 1.0).abs() > 0.02
        || (up.length() - 1.0).abs() > 0.02
        || !forward.dot(up).is_finite()
        || forward.dot(up).abs() > 0.02
    {
        return Err("invalid Bryce 2 perspective camera directions".into());
    }
    let forward = forward.normalized();
    let right = forward.cross(Vec3(0.0, 1.0, 0.0));
    let right = if right.length() > 1e-5 {
        right.normalized()
    } else {
        Vec3(1.0, 0.0, 0.0)
    };
    let level_up = right.cross(forward);
    let fov = be_f64(data, 0x33c) as f32 * 0.8;
    Ok(LegacyCamera {
        eye,
        forward,
        target: be_vec(data, 0x31c),
        fov: fov.to_degrees(),
        lens: CameraLens {
            roll: (-up.dot(right)).atan2(up.dot(level_up)).to_degrees(),
            shift: [le_f64(data, 0x354) as f32, le_f64(data, 0x35c) as f32],
            scale: [le_f64(data, 0x364) as f32, le_f64(data, 0x36c) as f32],
        },
    })
}

fn le_vec(data: &[u8], offset: usize) -> Vec3 {
    Vec3(
        le_f64(data, offset) as f32,
        le_f64(data, offset + 8) as f32,
        -le_f64(data, offset + 16) as f32,
    )
}

fn be_vec(data: &[u8], offset: usize) -> Vec3 {
    Vec3(
        be_f64(data, offset) as f32,
        be_f64(data, offset + 8) as f32,
        -be_f64(data, offset + 16) as f32,
    )
}

fn be_f64(data: &[u8], offset: usize) -> f64 {
    f64::from_be_bytes(data[offset..offset + 8].try_into().unwrap())
}

fn le_f64(data: &[u8], offset: usize) -> f64 {
    f64::from_le_bytes(data[offset..offset + 8].try_into().unwrap())
}

fn square_resolution(bytes: usize) -> Option<u32> {
    let area = bytes.checked_sub(HEIGHT_HEADER)?;
    [16, 32, 64, 128, 256, 512, 1024]
        .into_iter()
        .find(|side| 2 * (*side as usize).pow(2) == area)
}

fn height_header_matches(data: &[u8], side: u32) -> bool {
    let extent = f64::from_bits(u64::from_be_bytes(data[0x308..0x310].try_into().unwrap()));
    data[0x2ec..0x2f0] == [1, 0, 0, 0]
        && read_u32(&data[0x348..]) == side
        && (extent == f64::from(side - 1) || extent == f64::from(side))
}

fn read_chain(
    file: &mut File,
    file_size: u64,
    mut offset: u64,
    size: usize,
    prefix: usize,
) -> Result<Vec<u8>> {
    let mut data = Vec::with_capacity(prefix);
    let mut visited = HashSet::new();
    while data.len() < prefix {
        if offset < HEADER_END
            || offset + STREAM_HEADER as u64 > file_size
            || !visited.insert(offset)
        {
            return Err("invalid Bryce 2 resource block offset or cycle".into());
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut header = [0; STREAM_HEADER];
        file.read_exact(&mut header)?;
        let allocation = u64::from(read_u32(&header));
        let bytes = read_u32(&header[4..]) as usize;
        let next = u64::from(read_u32(&header[8..]));
        if bytes == 0
            || bytes > size - data.len()
            || offset + allocation > file_size
            || bytes as u64 + STREAM_HEADER as u64 > allocation
        {
            return Err("invalid Bryce 2 resource block length".into());
        }
        let end = prefix.min(data.len() + bytes);
        let start = data.len();
        data.resize(end, 0);
        file.read_exact(&mut data[start..])?;
        offset = next;
    }
    if prefix == size && offset != 0 {
        return Err("Bryce 2 resource has blocks beyond its declared length".into());
    }
    Ok(data)
}

fn read_u32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes(bytes[..4].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn decodes_big_endian_terrain_across_linked_blocks() {
        let mut data = vec![0; HEADER_END as usize];
        data[..7].copy_from_slice(b"CCmF - ");
        data[0x248] = 1;
        data[0x24c..0x250].copy_from_slice(&12_u32.to_be_bytes());
        data[0x250..0x254].copy_from_slice(&(HEADER_END as u32).to_be_bytes());

        let mut terrain = vec![0; HEIGHT_HEADER];
        terrain[0x2ec] = 1;
        terrain[0x308..0x310].copy_from_slice(&15_f64.to_be_bytes());
        terrain[0x348..0x34c].copy_from_slice(&16_u32.to_be_bytes());
        for sample in 0..256_u16 {
            terrain.extend_from_slice(&(sample * 257).to_be_bytes());
        }
        let mut entry = vec![1, 0, 0, 0];
        entry.extend_from_slice(&(terrain.len() as u32).to_be_bytes());
        entry.extend_from_slice(&(HEADER_END as u32 + 24).to_be_bytes());
        append_block(&mut data, &entry, 0);
        let cut = 0x305;
        let next = data.len() as u32 + 12 + cut as u32;
        append_block(&mut data, &terrain[..cut], next);
        append_block(&mut data, &terrain[cut..], 0);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let file =
            std::env::temp_dir().join(format!("bryce-heightmap-{}-{now}.brc", std::process::id()));
        fs::write(&file, &data).unwrap();
        let mut archive = LegacyScene::open(&file).unwrap();
        let map = archive.terrain(None).unwrap();
        assert_eq!((map.width, map.depth), (16, 16));
        assert_eq!(map.heights[0], 0.0);
        assert_eq!(map.heights[255], 1.0);
        assert_eq!(map.heights[64], 64.0 / 255.0);
        data[next as usize + 8..next as usize + 12].copy_from_slice(&next.to_be_bytes());
        fs::write(&file, &data).unwrap();
        assert!(LegacyScene::open(&file).unwrap().terrain(None).is_err());
        fs::remove_file(file).unwrap();
    }

    fn append_block(file: &mut Vec<u8>, payload: &[u8], next: u32) {
        let used = payload.len() as u32;
        file.extend_from_slice(&(used + 12).to_be_bytes());
        file.extend_from_slice(&used.to_be_bytes());
        file.extend_from_slice(&next.to_be_bytes());
        file.extend_from_slice(payload);
    }
}
