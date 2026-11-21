mod assets;
mod geometry;
mod iso;
mod legacy;
mod math;
mod objects;
mod render;
mod scene;
mod ui;

use iso::Disc;
use legacy::LegacyScene;
use scene::{Preset, Result, Scene, Terrain};
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const HELP: &str = "Terrain Studio
  terrain-studio [PROJECT.terrain|SCENE.BRC]
  terrain-studio render OUTPUT.png [--preset mirage|checker|mercury|alpine|canyon|coast]
                        [--scene PROJECT.terrain | --brc-scene SCENE.BRC]
                        [--seed N] [--size WIDTHxHEIGHT]
                        [--heightmap IMAGE | --iso BRYCE2.iso --map NAME.TIF
                         | --brc SCENE.BRC [--terrain-index ID]] [--draft]
  terrain-studio list-maps BRYCE2.iso
  terrain-studio list-brc-terrains SCENE.BRC
  terrain-studio convert-brc SCENE.BRC OUTPUT.terrain
  terrain-studio --version";

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error)
            if error
                .downcast_ref::<io::Error>()
                .is_some_and(|e| e.kind() == io::ErrorKind::BrokenPipe) =>
        {
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("terrain-studio: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let first = args.next();
    let rest: Vec<_> = args.collect();
    match first.as_deref().and_then(|v| v.to_str()) {
        Some("--help" | "-h") => writeln!(io::stdout().lock(), "{HELP}")?,
        Some("--version" | "-V") => writeln!(
            io::stdout().lock(),
            "terrain-studio {}",
            env!("CARGO_PKG_VERSION")
        )?,
        Some("list-maps") if rest.len() == 1 => {
            let mut out = io::stdout().lock();
            for name in Disc::open(Path::new(&rest[0]))?.names() {
                writeln!(out, "{name}")?;
            }
        }
        Some("list-brc-terrains") if rest.len() == 1 => {
            let mut out = io::stdout().lock();
            let source = LegacyScene::open(Path::new(&rest[0]))?;
            for item in source.maps() {
                writeln!(out, "{}\t{}x{}", item.index, item.side, item.side)?;
            }
        }
        Some("convert-brc") if rest.len() == 2 => {
            let imported = LegacyScene::open_project(Path::new(&rest[0]))?;
            imported.scene.save(Path::new(&rest[1]))?;
            writeln!(io::stdout().lock(), "{}", imported.summary())?;
        }
        Some("render") => render_command(&rest)?,
        Some(v)
            if v.starts_with('-')
                || matches!(v, "list-maps" | "list-brc-terrains" | "convert-brc") =>
        {
            return Err(format!("invalid command or option\n{HELP}").into())
        }
        _ if rest.is_empty() => ui::launch(first.map(PathBuf::from))?,
        _ => return Err(format!("unexpected arguments\n{HELP}").into()),
    }
    Ok(())
}

fn render_command(args: &[OsString]) -> Result<()> {
    let (output, options) = args
        .split_first()
        .ok_or("render requires an output PNG path")?;
    let output = Path::new(output);
    let mut preset = None;
    let mut seed = None;
    let mut size = None;
    let mut project = None;
    let mut legacy_project = None;
    let mut map = None;
    let mut iso = None;
    let mut map_name = None;
    let mut brc = None;
    let mut terrain_index = None;
    let mut antialias = true;
    let mut args = options.iter();
    while let Some(option) = args.next() {
        if option == "--draft" {
            antialias = false;
            continue;
        }
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {}", option.to_string_lossy()))?;
        let text = || value.to_str().ok_or("option value must be UTF-8");
        match option.to_str() {
            Some("--preset") => {
                preset = Some(Preset::from_slug(text()?).ok_or("unknown scene preset")?)
            }
            Some("--seed") => seed = Some(text()?.parse::<u32>()?),
            Some("--size") => {
                let (w, h) = text()?.split_once('x').ok_or("use WIDTHxHEIGHT for size")?;
                size = Some([w.parse()?, h.parse()?]);
            }
            Some("--scene") => project = Some(Path::new(value)),
            Some("--brc-scene") => legacy_project = Some(Path::new(value)),
            Some("--heightmap") => map = Some(Path::new(value)),
            Some("--iso") => iso = Some(Path::new(value)),
            Some("--map") => map_name = Some(text()?),
            Some("--brc") => brc = Some(Path::new(value)),
            Some("--terrain-index") => terrain_index = Some(text()?.parse::<usize>()?),
            _ => return Err(format!("unknown render option {}", option.to_string_lossy()).into()),
        }
    }
    let terrain_sources = [map.is_some(), iso.is_some(), brc.is_some()];
    if terrain_sources
        .into_iter()
        .filter(|present| *present)
        .count()
        > 1
        || iso.is_some() != map_name.is_some()
        || (terrain_index.is_some() && brc.is_none())
    {
        return Err(
            "choose --heightmap IMAGE, --iso ISO --map NAME, or --brc SCENE [--terrain-index ID]"
                .into(),
        );
    }
    if project.is_some() && legacy_project.is_some()
        || (project.is_some() || legacy_project.is_some()) && (preset.is_some() || seed.is_some())
        || legacy_project.is_some() && terrain_sources.contains(&true)
    {
        return Err("choose a native project, a Bryce 2 scene, or a preset and seed".into());
    }
    let default = if terrain_sources.contains(&true) {
        Preset::Alpine
    } else {
        Preset::Mirage
    };
    let mut scene = if let Some(path) = legacy_project {
        let imported = LegacyScene::open_project(path)?;
        eprintln!("{}", imported.summary());
        imported.scene
    } else if let Some(path) = project {
        Scene::open(path)?
    } else {
        preset.unwrap_or(default).scene(seed.unwrap_or(42))
    };
    let [width, height] = size.or(scene.output_size).unwrap_or([960, 720]);
    render::dimensions(width, height)?;
    if let Some(path) = map {
        scene.terrain = Terrain::import(path)?;
        scene.terrain_visible = true;
    }
    if let (Some(path), Some(name)) = (iso, map_name) {
        scene.terrain = Disc::open(path)?.terrain(name)?;
        scene.terrain_visible = true;
    }
    if let Some(path) = brc {
        scene.terrain = LegacyScene::open(path)?.terrain(terrain_index)?;
        scene.terrain_visible = true;
    }
    let image = render::render(&scene, width, height, antialias);
    render::save_png(output, &image)?;
    writeln!(
        io::stdout().lock(),
        "Rendered {width} × {height} to {}",
        output.display()
    )?;
    Ok(())
}
