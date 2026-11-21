use crate::scene::{Result, Terrain};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const SECTOR: usize = 2048;

pub struct Disc {
    file: File,
    length: u64,
    maps: Vec<Entry>,
}

struct Entry {
    name: String,
    offset: u64,
    size: usize,
    directory: bool,
}

impl Disc {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let length = file.metadata()?.len();
        let mut disc = Self {
            file,
            length,
            maps: Vec::new(),
        };
        for sector in 16..80 {
            let mut volume = [0; SECTOR];
            disc.file.seek(SeekFrom::Start((sector * SECTOR) as u64))?;
            disc.file.read_exact(&mut volume)?;
            if &volume[1..7] != b"CD001\x01" || volume[0] == 255 {
                break;
            }
            if volume[0] != 1 {
                continue;
            }
            if u16::from_le_bytes([volume[128], volume[129]]) as usize != SECTOR {
                return Err("disc uses an unsupported logical block size".into());
            }
            let root = entry(&volume[156..])?;
            let terrain = disc
                .directory(&root)?
                .into_iter()
                .find(|item| item.directory && item.name.eq_ignore_ascii_case("TERRAIN"))
                .ok_or("TERRAIN folder is missing from this ISO")?;
            disc.maps = disc
                .directory(&terrain)?
                .into_iter()
                .filter(|item| !item.directory && item.name.to_ascii_uppercase().ends_with(".TIF"))
                .collect();
            disc.maps
                .sort_unstable_by(|a, b| a.name.cmp(&b.name).then_with(|| a.offset.cmp(&b.offset)));
            disc.maps.dedup_by(|a, b| a.name == b.name);
            if disc.maps.is_empty() {
                return Err("no TIFF heightmaps found in the ISO".into());
            }
            return Ok(disc);
        }
        Err("no ISO 9660 primary volume found".into())
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.maps.iter().map(|item| item.name.as_str())
    }

    pub fn terrain(&mut self, name: &str) -> Result<Terrain> {
        let item = self
            .maps
            .iter()
            .find(|item| item.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("heightmap {name} is missing from the ISO"))?;
        let data = read_extent(&mut self.file, self.length, item, 64 * 1024 * 1024)?;
        Terrain::from_bytes(&data)
    }

    fn directory(&mut self, item: &Entry) -> Result<Vec<Entry>> {
        let data = read_extent(&mut self.file, self.length, item, 4 * 1024 * 1024)?;
        let mut records = Vec::new();
        for block in data.chunks(SECTOR) {
            let mut rest = block;
            while rest.first().is_some_and(|&length| length != 0) {
                records.push(entry(rest)?);
                rest = &rest[rest[0] as usize..];
            }
        }
        Ok(records)
    }
}

fn entry(bytes: &[u8]) -> Result<Entry> {
    let length = bytes.first().copied().unwrap_or(0) as usize;
    if length < 34 || length > bytes.len() {
        return Err("invalid ISO directory record size".into());
    }
    let record = &bytes[..length];
    let field = |i: usize| u32::from_le_bytes(record[i..i + 4].try_into().unwrap());
    let offset = u64::from(field(2)) * SECTOR as u64 + u64::from(record[1]) * SECTOR as u64;
    let id = record
        .get(33..33 + record[32] as usize)
        .ok_or("invalid ISO file identifier")?;
    let name = String::from_utf8_lossy(id);
    let name = name
        .split(';')
        .next()
        .unwrap_or_default()
        .trim_end_matches('.')
        .to_owned();
    Ok(Entry {
        name,
        offset,
        size: field(10) as usize,
        directory: record[25] & 2 != 0,
    })
}

fn read_extent(file: &mut File, length: u64, item: &Entry, limit: usize) -> Result<Vec<u8>> {
    if item.size > limit || item.offset + item.size as u64 > length {
        return Err("ISO extent exceeds file or size limit".into());
    }
    file.seek(SeekFrom::Start(item.offset))?;
    let mut data = vec![0; item.size];
    file.read_exact(&mut data)?;
    Ok(data)
}
