use crate::assets::{Bitmap, Mesh};
use crate::iso::Disc;
use crate::legacy::LegacyScene;
use crate::objects::{Material, Object, Pattern, Shape};
use crate::render;
use crate::scene::{Look, Preset, Result, Scene, Terrain, MAX_OBJECTS};
use eframe::egui::{
    self, Align, Color32, ColorImage, Context, DragValue, Frame, Key, Layout, Margin, Modifiers,
    PointerButton, Rect, RichText, Sense, Slider, Stroke, TextureHandle, TextureOptions, Ui, Vec2,
};
use image::RgbImage;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;

const BG: Color32 = Color32::from_rgb(14, 17, 24);
const PANEL: Color32 = Color32::from_rgb(22, 27, 37);
const BORDER: Color32 = Color32::from_rgb(47, 54, 70);
const INK: Color32 = Color32::from_rgb(228, 233, 244);
const MUTED: Color32 = Color32::from_rgb(151, 163, 186);
const ACCENT: Color32 = Color32::from_rgb(125, 216, 222);
const HISTORY_LIMIT: usize = 25;

pub fn launch(path: Option<PathBuf>) -> Result<()> {
    let (scene, project, notice) = match path {
        Some(path) => open_scene_file(path)?,
        None => (Preset::Mirage.scene(42), None, String::new()),
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id("terrain-studio")
            .with_inner_size([1200.0, 820.0])
            .with_min_inner_size([760.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Terrain Studio",
        options,
        Box::new(|cc| {
            install_style(&cc.egui_ctx);
            let mut editor = Editor::new(scene, project);
            editor.status(notice, false);
            Ok(Box::new(editor))
        }),
    )
    .map_err(|error| error.to_string().into())
}

fn open_scene_file(path: PathBuf) -> Result<(Scene, Option<PathBuf>, String)> {
    if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("brc"))
    {
        let imported = LegacyScene::open_project(&path)?;
        let notice = imported.summary();
        Ok((imported.scene, None, notice))
    } else {
        Ok((Scene::open(&path)?, Some(path), String::new()))
    }
}

fn install_style(ctx: &Context) {
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = Vec2::new(9.0, 8.0);
    style.spacing.button_padding = Vec2::new(12.0, 6.0);
    style.spacing.slider_width = 120.0;
    style.visuals = egui::Visuals::dark();
    style.visuals.override_text_color = Some(INK);
    style.visuals.panel_fill = PANEL;
    style.visuals.window_fill = PANEL;
    style.visuals.extreme_bg_color = BG;
    style.visuals.faint_bg_color = Color32::from_rgb(28, 34, 46);
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(34, 42, 56);
    style.visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(34, 42, 56);
    style.visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(47, 60, 75);
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    style.visuals.selection.bg_fill = Color32::from_rgb(40, 73, 82);
    style.visuals.selection.stroke = Stroke::new(1.0_f32, ACCENT);
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    ctx.set_style(style);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Objects,
    Terrain,
    Sky,
    Camera,
}

struct Snapshot {
    scene: Scene,
    version: u64,
}

enum OpenAction {
    Project(PathBuf),
    Preset(Preset),
    Empty,
    Close,
}

struct Editor {
    state: Snapshot,
    saved_version: u64,
    next_version: u64,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    grouping_drag: bool,
    project: Option<PathBuf>,
    selected: Option<usize>,
    tab: Tab,
    map_mode: bool,
    terrain_edit: Option<usize>,
    last_brush: Option<egui::Pos2>,
    brush_radius: f32,
    brush_strength: f32,
    heightmap: Option<TextureHandle>,
    disc: Option<Disc>,
    legacy: Option<LegacyScene>,
    pending_open: Option<OpenAction>,
    allow_close: bool,
    export_dialog: bool,
    export_job: Option<Receiver<std::result::Result<PathBuf, String>>>,
    export_size: [u32; 2],
    export_antialias: bool,
    notice: String,
    notice_error: bool,
    preview: Preview,
}

impl Editor {
    fn new(scene: Scene, project: Option<PathBuf>) -> Self {
        let selected = (!scene.objects.is_empty()).then_some(0);
        let export_size = scene.output_size.unwrap_or([1280, 960]);
        Self {
            state: Snapshot { scene, version: 0 },
            saved_version: 0,
            next_version: 1,
            undo: Vec::new(),
            redo: Vec::new(),
            grouping_drag: false,
            project,
            selected,
            tab: Tab::Objects,
            map_mode: false,
            terrain_edit: None,
            last_brush: None,
            brush_radius: 0.063,
            brush_strength: 0.034,
            heightmap: None,
            disc: None,
            legacy: None,
            pending_open: None,
            allow_close: false,
            export_dialog: false,
            export_job: None,
            export_size,
            export_antialias: true,
            notice: String::new(),
            notice_error: false,
            preview: Preview::default(),
        }
    }

    fn status(&mut self, msg: impl Into<String>, error: bool) {
        self.notice = msg.into();
        self.notice_error = error;
    }

    fn scene_changed(&mut self) {
        self.heightmap = None;
        self.terrain_edit = self.terrain_edit.filter(|&index| {
            self.state
                .scene
                .objects
                .get(index)
                .is_some_and(|object| object.heightmap.is_some())
        });
        self.selected = self
            .selected
            .filter(|&i| i < self.state.scene.objects.len());
        self.preview.invalidate();
    }

    fn changed(&mut self, before: Snapshot, pointer_down: bool) {
        if !(pointer_down && self.grouping_drag) {
            if self.undo.len() == HISTORY_LIMIT {
                self.undo.remove(0);
            }
            self.undo.push(before);
        }
        self.grouping_drag = pointer_down;
        self.redo.clear();
        self.state.version = self.next_version;
        self.next_version += 1;
        self.scene_changed();
    }

    fn history(&mut self, backwards: bool) {
        let (from, to) = if backwards {
            (&mut self.undo, &mut self.redo)
        } else {
            (&mut self.redo, &mut self.undo)
        };
        if let Some(previous) = from.pop() {
            self.terrain_edit = None;
            to.push(std::mem::replace(&mut self.state, previous));
            self.scene_changed();
        }
    }

    fn ask_open(&mut self, ctx: &Context, action: OpenAction) {
        if self.state.version == self.saved_version {
            self.perform_open(ctx, action);
        } else {
            self.pending_open = Some(action);
        }
    }

    fn perform_open(&mut self, ctx: &Context, action: OpenAction) {
        let (scene, project, notice) = match action {
            OpenAction::Close => {
                self.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
            OpenAction::Preset(preset) => {
                (preset.scene(self.state.scene.seed), None, String::new())
            }
            OpenAction::Empty => {
                let mut scene = Preset::Checker.scene(42);
                scene.objects.clear();
                (scene, None, String::new())
            }
            OpenAction::Project(path) => match open_scene_file(path) {
                Ok(loaded) => loaded,
                Err(error) => {
                    self.status(format!("Could not open project: {error}"), true);
                    return;
                }
            },
        };
        self.export_size = scene.output_size.unwrap_or([1280, 960]);
        self.state = Snapshot {
            scene,
            version: self.next_version,
        };
        self.saved_version = self.next_version;
        self.next_version += 1;
        self.project = project;
        self.undo.clear();
        self.redo.clear();
        self.selected = (!self.state.scene.objects.is_empty()).then_some(0);
        self.map_mode = false;
        self.terrain_edit = None;
        self.status(notice, false);
        self.scene_changed();
    }

    fn save(&mut self, save_as: bool) -> bool {
        let selected = if save_as || self.project.is_none() {
            match choose_file(Dialog::SaveProject) {
                Ok(path) => path,
                Err(error) => {
                    self.status(error.to_string(), true);
                    return false;
                }
            }
        } else {
            self.project.clone()
        };
        let Some(path) = selected else {
            return false;
        };
        match self.state.scene.save(&path) {
            Ok(()) => {
                self.saved_version = self.state.version;
                self.status(format!("Saved {}", filename(&path)), false);
                self.project = Some(path);
                true
            }
            Err(error) => {
                self.status(format!("Could not save project: {error}"), true);
                false
            }
        }
    }

    fn toolbar(&mut self, ctx: &Context) {
        egui::TopBottomPanel::top("toolbar")
            .frame(
                Frame::new()
                    .fill(PANEL)
                    .inner_margin(Margin::symmetric(18, 12)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let (icon, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
                    let tip = icon.center_top() + Vec2::new(-2.0, 1.0);
                    let left = icon.left_bottom() - Vec2::new(-1.0, 1.0);
                    let center = icon.center_bottom() - Vec2::new(-3.0, 1.0);
                    let right = icon.right_bottom() - Vec2::new(1.0, 1.0);
                    ui.painter().add(egui::Shape::convex_polygon(
                        vec![tip, left, center],
                        ACCENT,
                        Stroke::NONE,
                    ));
                    ui.painter().add(egui::Shape::convex_polygon(
                        vec![tip, center, right],
                        Color32::from_rgb(191, 115, 237),
                        Stroke::NONE,
                    ));
                    ui.label(RichText::new("Terrain Studio").size(20.0).strong());
                    ui.add_space(12.0);
                    ui.menu_button("File", |ui| {
                        if ui.button("New empty scene").clicked() {
                            ui.close_menu();
                            self.ask_open(ctx, OpenAction::Empty);
                        }
                        if ui.button("Open project…").clicked() {
                            ui.close_menu();
                            self.open_dialog(ctx);
                        }
                        if ui.button("Save project").clicked() {
                            ui.close_menu();
                            self.save(false);
                        }
                        if ui.button("Save project as…").clicked() {
                            ui.close_menu();
                            self.save(true);
                        }
                    });
                    ui.menu_button("Starter scenes", |ui| {
                        for preset in Preset::ALL {
                            if ui.button(preset.name()).clicked() {
                                ui.close_menu();
                                self.ask_open(ctx, OpenAction::Preset(preset));
                            }
                        }
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let button =
                            egui::Button::new(RichText::new("Render PNG…").strong().color(BG))
                                .fill(ACCENT);
                        if ui.add_enabled(self.export_job.is_none(), button).clicked() {
                            self.export_dialog = true;
                        }
                        if ui
                            .add_enabled(!self.redo.is_empty(), egui::Button::new("Redo"))
                            .clicked()
                        {
                            self.history(false);
                        }
                        if ui
                            .add_enabled(!self.undo.is_empty(), egui::Button::new("Undo"))
                            .clicked()
                        {
                            self.history(true);
                        }
                    });
                });
            });
    }

    fn open_dialog(&mut self, ctx: &Context) {
        match choose_file(Dialog::OpenProject) {
            Ok(Some(path)) => self.ask_open(ctx, OpenAction::Project(path)),
            Ok(None) => {}
            Err(error) => self.status(error.to_string(), true),
        }
    }

    fn inspector(&mut self, ctx: &Context) -> bool {
        let mut changed = false;
        egui::SidePanel::right("inspector")
            .resizable(false)
            .exact_width(348.0)
            .frame(Frame::new().fill(PANEL).inner_margin(Margin::same(16)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    for (tab, label) in [
                        (Tab::Objects, "Objects"),
                        (Tab::Terrain, "Terrain"),
                        (Tab::Sky, "Sky & water"),
                        (Tab::Camera, "Camera"),
                    ] {
                        if ui.selectable_label(self.tab == tab, label).clicked() {
                            self.tab = tab;
                            self.map_mode = tab == Tab::Terrain;
                        }
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        changed = match self.tab {
                            Tab::Objects => self.objects(ui),
                            Tab::Terrain => self.terrain(ui),
                            Tab::Sky => sky(ui, &mut self.state.scene),
                            Tab::Camera => camera(ui, &mut self.state.scene),
                        };
                    });
            });
        changed
    }

    fn objects(&mut self, ui: &mut Ui) -> bool {
        let scene = &mut self.state.scene;
        let mut changed = false;
        let mut import = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Scene objects").size(16.0).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_enabled_ui(scene.objects.len() < MAX_OBJECTS, |ui| {
                    ui.menu_button("Add object", |ui| {
                        for shape in Shape::ALL {
                            if ui.button(shape.name()).clicked() {
                                ui.close_menu();
                                scene.objects.push(Object::new(shape));
                                self.selected = Some(scene.objects.len() - 1);
                                changed = true;
                            }
                        }
                        ui.separator();
                        for (dialog, label) in [
                            (Dialog::OpenMesh, "Import OBJ mesh…"),
                            (Dialog::OpenPicture, "Import image panel…"),
                        ] {
                            if ui.button(label).clicked() {
                                ui.close_menu();
                                import = Some(dialog);
                            }
                        }
                    });
                });
            });
        });
        if let Some(dialog) = import {
            let add = (|| -> Result<Option<Object>> {
                let Some(path) = choose_file(dialog)? else {
                    return Ok(None);
                };
                let stem: String = path
                    .file_stem()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy()
                    .chars()
                    .take(80)
                    .collect();
                let object = match dialog {
                    Dialog::OpenMesh => Object::mesh(&stem, Mesh::import(&path)?),
                    Dialog::OpenPicture => Object::picture(&stem, Bitmap::import(&path)?),
                    _ => unreachable!(),
                };
                Ok(Some(object))
            })();
            match add {
                Ok(Some(object)) if scene.can_add(&object) => {
                    scene.objects.push(object);
                    self.selected = Some(scene.objects.len() - 1);
                    self.notice.clear();
                    changed = true;
                }
                Ok(Some(_)) => {
                    self.notice = "Scene image or geometry limit reached.".into();
                    self.notice_error = true;
                }
                Ok(None) => {}
                Err(error) => {
                    self.notice = format!("Could not import object: {error}");
                    self.notice_error = true;
                }
            }
        }
        if scene.objects.is_empty() {
            ui.label(RichText::new("Add a shape to start composing.").color(MUTED));
        } else {
            egui::ScrollArea::vertical()
                .id_salt("object_list")
                .max_height(156.0)
                .show(ui, |ui| {
                    for (index, object) in scene.objects.iter().enumerate() {
                        let color = Color32::from_rgb(
                            (object.material.color[0] * 255.0) as u8,
                            (object.material.color[1] * 255.0) as u8,
                            (object.material.color[2] * 255.0) as u8,
                        );
                        ui.horizontal(|ui| {
                            let (rect, _) =
                                ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                            ui.painter().rect_filled(rect, 2.0, color);
                            if ui
                                .add(
                                    egui::Button::new(&object.name)
                                        .selected(self.selected == Some(index))
                                        .truncate(),
                                )
                                .on_hover_text(object.shape.name())
                                .clicked()
                            {
                                self.selected = Some(index);
                            }
                        });
                    }
                });
        }

        if let Some(index) = self.selected {
            ui.separator();
            let object = &mut scene.objects[index];
            changed |= ui
                .add(
                    egui::TextEdit::singleline(&mut object.name)
                        .char_limit(80)
                        .desired_width(f32::INFINITY)
                        .hint_text("Object name"),
                )
                .changed();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        scene.can_add(&scene.objects[index]),
                        egui::Button::new("Duplicate"),
                    )
                    .clicked()
                {
                    let mut object = scene.objects[index].clone();
                    object.position[0] = (object.position[0] + 0.08).min(128.0);
                    scene.objects.push(object);
                    self.selected = Some(scene.objects.len() - 1);
                    changed = true;
                }
                if ui.button("Delete").clicked() {
                    scene.objects.remove(index);
                    self.terrain_edit = None;
                    self.selected = None;
                    changed = true;
                }
            });
            if let Some(index) = self.selected {
                let object = &mut scene.objects[index];
                ui.separator();
                ui.label(RichText::new("Transform").strong());
                changed |= axes(ui, "Position", &mut object.position, -128.0..=128.0, 0.01);
                changed |= axes(ui, "Scale", &mut object.scale, 0.005..=4.0, 0.01);
                changed |= axes(ui, "Rotation", &mut object.rotation, -180.0..=180.0, 1.0);
                ui.separator();
                if object.shape == Shape::Mesh {
                    changed |= ui
                        .checkbox(&mut object.smooth_mesh, "Smooth mesh normals")
                        .changed();
                }
                if object.heightmap.is_some() {
                    if ui.button("Edit elevation").clicked() {
                        self.terrain_edit = Some(index);
                        self.heightmap = None;
                        self.last_brush = None;
                        self.tab = Tab::Terrain;
                        self.map_mode = true;
                    }
                    changed |= ui
                        .checkbox(&mut object.natural_terrain, "Natural terrain material")
                        .changed();
                }
                ui.label(RichText::new("Material").strong());
                if object.shape == Shape::Image {
                    changed |= slider(
                        ui,
                        &mut object.material.reflectivity,
                        0.0..=1.0,
                        "Reflection",
                    );
                    changed |= slider(ui, &mut object.material.roughness, 0.0..=1.0, "Roughness");
                } else if !object.natural_terrain {
                    changed |= material(ui, &mut object.material, true);
                }
            }
        }
        changed
    }

    fn terrain(&mut self, ui: &mut Ui) -> bool {
        let objects = &self.state.scene.objects;
        if objects.iter().any(|object| object.heightmap.is_some()) {
            let current = self
                .terrain_edit
                .and_then(|index| objects.get(index))
                .map_or("Base terrain", |object| object.name.as_str());
            let mut edit = self.terrain_edit;
            egui::ComboBox::from_id_salt("terrain_to_edit")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut edit, None, "Base terrain");
                    for (index, object) in objects.iter().enumerate() {
                        if object.heightmap.is_some() {
                            ui.selectable_value(&mut edit, Some(index), &object.name);
                        }
                    }
                });
            if edit != self.terrain_edit {
                self.terrain_edit = edit;
                self.heightmap = None;
                self.last_brush = None;
            }
            if let Some(index) = self.terrain_edit {
                return self.placed_terrain(ui, index);
            }
        }
        let scene = &mut self.state.scene;
        let mut changed = ui
            .checkbox(&mut scene.terrain_visible, "Show terrain in render")
            .changed();
        changed |= slider(ui, &mut scene.relief, 0.05..=2.0, "Relief");
        ui.horizontal(|ui| {
            if ui.button("Import image…").clicked() {
                match choose_file(Dialog::OpenMap)
                    .and_then(|path| path.map(|p| Terrain::import(&p)).transpose())
                {
                    Ok(Some(map)) => {
                        scene.terrain = map;
                        scene.terrain_visible = true;
                        changed = true;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        self.notice = format!("Could not import heightmap: {error}");
                        self.notice_error = true;
                    }
                }
            }
            if ui.button("Open Bryce ISO…").clicked() {
                match choose_file(Dialog::OpenIso)
                    .and_then(|path| path.map(|p| Disc::open(&p)).transpose())
                {
                    Ok(Some(disc)) => self.disc = Some(disc),
                    Ok(None) => {}
                    Err(error) => {
                        self.notice = format!("Could not read ISO: {error}");
                        self.notice_error = true;
                    }
                }
            }
        });

        if let Some(disc) = &mut self.disc {
            let mut chosen = None;
            egui::ComboBox::from_id_salt("disc_maps")
                .selected_text("Import a disc heightmap")
                .show_ui(ui, |ui| {
                    for name in disc.names() {
                        if ui.selectable_label(false, name).clicked() {
                            chosen = Some(name.to_owned());
                        }
                    }
                });
            if let Some(name) = chosen {
                match disc.terrain(&name) {
                    Ok(map) => {
                        scene.terrain = map;
                        scene.terrain_visible = true;
                        self.notice = format!("Imported {name} from ISO");
                        self.notice_error = false;
                        changed = true;
                    }
                    Err(error) => {
                        self.notice = format!("Could not decode {name}: {error}");
                        self.notice_error = true;
                    }
                }
            }
        }

        if ui.button("Open Bryce 2 scene…").clicked() {
            let opened = choose_file(Dialog::OpenBryceScene)
                .and_then(|path| path.map(|p| LegacyScene::open(&p)).transpose());
            match opened {
                Ok(Some(source)) if source.maps().is_empty() => {
                    self.notice = "This Bryce 2 scene has no supported terrain heightmaps.".into();
                    self.notice_error = true;
                }
                Ok(Some(source)) => {
                    self.notice = format!("Found {} terrain heightmaps.", source.maps().len());
                    self.notice_error = false;
                    self.legacy = Some(source);
                }
                Ok(None) => {}
                Err(error) => {
                    self.notice = format!("Could not read Bryce 2 scene: {error}");
                    self.notice_error = true;
                }
            }
        }
        if let Some(source) = &mut self.legacy {
            let mut selected = None;
            egui::ComboBox::from_id_salt("legacy_terrains")
                .selected_text("Import a Bryce 2 heightmap")
                .show_ui(ui, |ui| {
                    for map in source.maps() {
                        let caption =
                            format!("Resource {} · {} × {}", map.index, map.side, map.side);
                        if ui.selectable_label(false, caption).clicked() {
                            selected = Some(map.index);
                        }
                    }
                });
            if let Some(index) = selected {
                match source.terrain(Some(index)) {
                    Ok(map) => {
                        scene.terrain = map;
                        scene.terrain_visible = true;
                        self.notice = format!("Imported Bryce 2 heightmap {index}.");
                        self.notice_error = false;
                        changed = true;
                    }
                    Err(error) => {
                        self.notice = format!("Could not import heightmap {index}: {error}");
                        self.notice_error = true;
                    }
                }
            }
        }

        if ui.button("Save heightmap PNG…").clicked() {
            match choose_file(Dialog::SaveMap).and_then(|path| {
                path.map(|p| scene.terrain.save_map(&p).map(|()| p))
                    .transpose()
            }) {
                Ok(Some(path)) => {
                    self.notice = format!("Saved {}", filename(&path));
                    self.notice_error = false;
                }
                Ok(None) => {}
                Err(error) => {
                    self.notice = format!("Could not save heightmap: {error}");
                    self.notice_error = true;
                }
            }
        }
        ui.separator();
        ui.label(RichText::new("Elevation brush").strong());
        slider(ui, &mut self.brush_radius, 0.008..=0.22, "Radius");
        slider(ui, &mut self.brush_strength, 0.005..=0.16, "Strength");
        if ui.button("Smooth heightmap").clicked() {
            scene.terrain.smooth();
            changed = true;
        }
        ui.label(RichText::new("Paint to raise. Hold Shift to lower.").color(MUTED));
        ui.separator();
        ui.label(RichText::new("Generate terrain").strong());
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("landform")
                .selected_text(scene.look.name())
                .show_ui(ui, |ui| {
                    for look in Look::ALL {
                        changed |= ui
                            .selectable_value(&mut scene.look, look, look.name())
                            .changed();
                    }
                });
            changed |= ui
                .add(DragValue::new(&mut scene.seed).prefix("Seed "))
                .changed();
        });
        if ui.button("Regenerate heightmap").clicked() {
            scene.terrain = Terrain::generate(scene.look, scene.seed);
            scene.terrain_visible = true;
            changed = true;
        }
        ui.separator();
        let mut custom = scene.terrain_color.is_some();
        if ui
            .checkbox(&mut custom, "Custom terrain material")
            .changed()
        {
            scene.terrain_color = custom.then(|| Material::solid([0.52, 0.32, 0.22]));
            changed = true;
        }
        if let Some(material_value) = &mut scene.terrain_color {
            changed |= material(ui, material_value, true);
        } else {
            changed |= slider(ui, &mut scene.snowline, 0.0..=1.0, "Snowline");
        }
        changed
    }

    fn placed_terrain(&mut self, ui: &mut Ui, index: usize) -> bool {
        let object = &mut self.state.scene.objects[index];
        let Some(heightmap) = &mut object.heightmap else {
            return false;
        };
        let map = &**heightmap;
        ui.label(format!("{} × {} samples", map.width, map.depth));
        if ui.button("Save heightmap PNG…").clicked() {
            match choose_file(Dialog::SaveMap)
                .and_then(|path| path.map(|p| map.save_map(&p).map(|()| p)).transpose())
            {
                Ok(Some(path)) => self.status(format!("Saved {}", filename(&path)), false),
                Ok(None) => {}
                Err(error) => self.status(format!("Could not save heightmap: {error}"), true),
            }
        }
        ui.separator();
        ui.label(RichText::new("Elevation brush").strong());
        slider(ui, &mut self.brush_radius, 0.008..=0.22, "Radius");
        slider(ui, &mut self.brush_strength, 0.005..=0.16, "Strength");
        ui.label(RichText::new("Paint to raise. Hold Shift to lower.").color(MUTED));
        if ui.button("Smooth heightmap").clicked() {
            if let Some(map) = &mut self.state.scene.objects[index].heightmap {
                Arc::make_mut(map).smooth();
                return true;
            }
        }
        false
    }

    fn viewport(&mut self, ctx: &Context) -> bool {
        let mut changed = false;
        egui::CentralPanel::default()
            .frame(Frame::new().fill(BG).inner_margin(Margin::same(18)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Workspace").size(15.0).strong());
                    ui.add_space(6.0);
                    ui.selectable_value(&mut self.map_mode, false, "Camera view");
                    if ui
                        .selectable_value(&mut self.map_mode, true, "Heightmap")
                        .clicked()
                    {
                        self.tab = Tab::Terrain;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let instructions = if self.map_mode {
                            "Drag to paint · Shift to lower"
                        } else {
                            "Click to select · Drag to orbit · Scroll to zoom"
                        };
                        ui.label(RichText::new(instructions).color(MUTED));
                    });
                });
                ui.add_space(7.0);
                let available = ui.available_size().max(Vec2::splat(1.0));
                let (area, _) = ui.allocate_exact_size(available, Sense::hover());
                if self.map_mode {
                    let scene = &self.state.scene;
                    let terrain = self
                        .terrain_edit
                        .and_then(|index| scene.objects[index].heightmap.as_deref())
                        .unwrap_or(&scene.terrain);
                    let aspect = terrain.width as f32 / terrain.depth as f32;
                    let view = fit(area, aspect);
                    if self.heightmap.is_none() {
                        let pixels = terrain
                            .heights
                            .iter()
                            .map(|v| Color32::from_gray((v * 255.0).round() as u8))
                            .collect();
                        let picture = ColorImage {
                            size: [terrain.width, terrain.depth],
                            pixels,
                        };
                        self.heightmap =
                            Some(ctx.load_texture("heightmap", picture, TextureOptions::LINEAR));
                    }
                    if let Some(texture) = &self.heightmap {
                        paint(ui, view, texture);
                    }
                    let response =
                        ui.interact(view, ui.id().with("paint"), Sense::click_and_drag());
                    if let Some(point) = response
                        .hover_pos()
                        .or_else(|| response.interact_pointer_pos())
                        .filter(|p| view.contains(*p))
                    {
                        let radii = view.size() * self.brush_radius;
                        ui.painter().add(egui::Shape::ellipse_stroke(
                            point,
                            radii,
                            Stroke::new(1.4_f32, ACCENT),
                        ));
                        if response.is_pointer_button_down_on() {
                            let uv = egui::pos2(
                                (point.x - view.min.x) / view.width(),
                                (point.y - view.min.y) / view.height(),
                            );
                            let elapsed = self
                                .last_brush
                                .map_or(f32::INFINITY, |last| uv.distance(last));
                            if elapsed > 0.001 {
                                let raise = if ctx.input(|input| input.modifiers.shift) {
                                    -1.0
                                } else {
                                    1.0
                                };
                                let scene = &mut self.state.scene;
                                let terrain = match self
                                    .terrain_edit
                                    .and_then(|index| scene.objects[index].heightmap.as_mut())
                                {
                                    Some(map) => Arc::make_mut(map),
                                    None => &mut scene.terrain,
                                };
                                let last = self.last_brush.unwrap_or(uv);
                                let steps = (last.distance(uv) / (self.brush_radius * 0.38))
                                    .ceil()
                                    .clamp(1.0, 140.0)
                                    as usize;
                                for i in 1..=steps {
                                    let p = last.lerp(uv, i as f32 / steps as f32);
                                    terrain.brush(
                                        p.x,
                                        p.y,
                                        self.brush_radius,
                                        self.brush_strength * raise,
                                    );
                                }
                                self.last_brush = Some(uv);
                                changed = true;
                            }
                        }
                    }
                    if !response.is_pointer_button_down_on() {
                        self.last_brush = None;
                    }
                } else {
                    let aspect = self.export_size[0] as f32 / self.export_size[1] as f32;
                    let view = fit(area, aspect);
                    let resolution = (view.width() * ctx.pixels_per_point()).clamp(96.0, 860.0);
                    self.preview.update(
                        ctx,
                        &self.state.scene,
                        resolution as u32,
                        (resolution / aspect).clamp(24.0, 860.0) as u32,
                    );
                    if let Some(texture) = &self.preview.texture {
                        paint(ui, view, texture);
                    } else {
                        ui.painter().rect_filled(view, 6.0, PANEL);
                        ui.painter().text(
                            view.center(),
                            egui::Align2::CENTER_CENTER,
                            "Rendering preview…",
                            egui::FontId::proportional(16.0),
                            MUTED,
                        );
                    }
                    let response =
                        ui.interact(view, ui.id().with("orbit"), Sense::click_and_drag());
                    if let Some(point) = response
                        .interact_pointer_pos()
                        .filter(|_| response.clicked())
                    {
                        let x = (point.x - view.min.x) / view.width();
                        let y = (point.y - view.min.y) / view.height();
                        self.selected = render::pick(&self.state.scene, aspect, x, y);
                        self.tab = Tab::Objects;
                    }
                    let camera = &mut self.state.scene.camera;
                    if response.dragged_by(PointerButton::Primary) {
                        let delta = response.drag_delta();
                        camera.heading =
                            (camera.heading - delta.x * 0.30 + 180.0).rem_euclid(360.0) - 180.0;
                        camera.elevation = (camera.elevation + delta.y * 0.26).clamp(-25.0, 88.0);
                        changed = delta != Vec2::ZERO;
                    }
                    if response.hovered() {
                        let delta = ctx.input(|input| input.smooth_scroll_delta.y);
                        if delta.abs() > 0.01 {
                            let distance =
                                (camera.distance * (-delta * 0.0023).exp()).clamp(0.05, 32.0);
                            changed |= distance != camera.distance;
                            camera.distance = distance;
                        }
                    }
                }
            });
        changed
    }

    fn dialogs(&mut self, ctx: &Context) {
        if self.pending_open.is_some() {
            let mut answer = None;
            let modal = egui::Modal::new(egui::Id::new("unsaved")).show(ctx, |ui| {
                ui.heading("Save your changes?");
                ui.label("The current scene has unsaved edits.");
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        answer = Some(true);
                    }
                    if ui.button("Discard").clicked() {
                        answer = Some(false);
                    }
                    if ui.button("Cancel").clicked() {
                        self.pending_open = None;
                    }
                });
            });
            if modal.should_close() {
                self.pending_open = None;
            }
            if let Some(save) = answer {
                if !save || self.save(false) {
                    if let Some(action) = self.pending_open.take() {
                        self.perform_open(ctx, action);
                    }
                }
            }
        }
        if self.export_dialog {
            let mut output = false;
            let modal = egui::Modal::new(egui::Id::new("export")).show(ctx, |ui| {
                ui.heading("Render PNG");
                ui.horizontal(|ui| {
                    ui.label("Width");
                    ui.add(
                        DragValue::new(&mut self.export_size[0])
                            .range(16..=4096)
                            .suffix(" px"),
                    );
                    ui.label("Height");
                    ui.add(
                        DragValue::new(&mut self.export_size[1])
                            .range(16..=4096)
                            .suffix(" px"),
                    );
                });
                ui.checkbox(&mut self.export_antialias, "Antialias edges");
                ui.horizontal(|ui| {
                    if ui.button("Choose destination…").clicked() {
                        output = true;
                    }
                    if ui.button("Cancel").clicked() {
                        self.export_dialog = false;
                    }
                });
            });
            if modal.should_close() {
                self.export_dialog = false;
            }
            if output {
                match choose_file(Dialog::SaveRender) {
                    Ok(Some(path)) => {
                        self.start_export(ctx, path);
                        self.export_dialog = false;
                    }
                    Ok(None) => {}
                    Err(error) => self.status(error.to_string(), true),
                }
            }
        }
    }

    fn start_export(&mut self, ctx: &Context, path: PathBuf) {
        let scene = self.state.scene.clone();
        let [w, h] = self.export_size;
        let aa = self.export_antialias;
        let wake = ctx.clone();
        let (send, receive) = mpsc::channel();
        self.export_job = Some(receive);
        self.status(format!("Rendering {w} × {h}…"), false);
        std::thread::spawn(move || {
            let result = render::dimensions(w, h)
                .and_then(|()| {
                    let image = render::render(&scene, w, h, aa);
                    render::save_png(&path, &image)?;
                    Ok(path)
                })
                .map_err(|error| error.to_string());
            let _ = send.send(result);
            wake.request_repaint();
        });
    }

    fn export_status(&mut self) {
        if let Some(job) = &self.export_job {
            match job.try_recv() {
                Ok(Ok(path)) => {
                    self.export_job = None;
                    self.status(format!("Rendered to {}", path.display()), false);
                }
                Ok(Err(error)) => {
                    self.export_job = None;
                    self.status(format!("Render failed: {error}"), true);
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.export_job = None;
                    self.status("Render was interrupted.", true);
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
    }

    fn status_bar(&self, ctx: &Context) {
        egui::TopBottomPanel::bottom("status")
            .frame(
                Frame::new()
                    .fill(PANEL)
                    .inner_margin(Margin::symmetric(18, 8)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let dirty = self.state.version != self.saved_version;
                    let document = self
                        .project
                        .as_deref()
                        .map_or_else(|| "Untitled".into(), filename);
                    ui.label(RichText::new(document).strong());
                    if dirty {
                        ui.label(RichText::new("Unsaved").color(ACCENT));
                    }
                    let color = if self.notice_error {
                        Color32::from_rgb(255, 157, 147)
                    } else {
                        MUTED
                    };
                    if !self.notice.is_empty() {
                        ui.add(
                            egui::Label::new(RichText::new(&self.notice).color(color)).truncate(),
                        )
                        .on_hover_text(&self.notice);
                    }
                });
            });
    }
}

impl eframe::App for Editor {
    fn update(&mut self, ctx: &Context, _: &mut eframe::Frame) {
        if ctx.input(|input| input.viewport().close_requested())
            && !self.allow_close
            && (self.state.version != self.saved_version || self.pending_open.is_some())
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.pending_open = Some(OpenAction::Close);
        }

        if self.pending_open.is_none() && !self.export_dialog {
            let [open, save, save_as, undo, redo] = ctx.input_mut(|input| {
                let shift_command = Modifiers::COMMAND | Modifiers::SHIFT;
                let save_as = input.consume_key(shift_command, Key::S);
                let redo = input.consume_key(shift_command, Key::Z)
                    || input.consume_key(Modifiers::COMMAND, Key::Y);
                [
                    input.consume_key(Modifiers::COMMAND, Key::O),
                    input.consume_key(Modifiers::COMMAND, Key::S),
                    save_as,
                    input.consume_key(Modifiers::COMMAND, Key::Z),
                    redo,
                ]
            });
            if open {
                self.open_dialog(ctx);
            }
            if undo || redo {
                self.history(!redo);
            }
            if save_as || save {
                self.save(save_as);
            }
        }

        self.export_status();
        self.toolbar(ctx);
        self.status_bar(ctx);
        let before = Snapshot {
            scene: self.state.scene.clone(),
            version: self.state.version,
        };
        let edited = self.inspector(ctx) | self.viewport(ctx);
        let pointer_down = ctx.input(|input| input.pointer.primary_down());
        if edited {
            self.changed(before, pointer_down);
            ctx.request_repaint();
        } else if !pointer_down {
            self.grouping_drag = false;
        }
        self.dialogs(ctx);
    }
}

#[derive(Default)]
struct Preview {
    texture: Option<TextureHandle>,
    generation: u64,
    target: (u32, u32),
    drawn: Option<u64>,
    job: Option<Receiver<(u64, RgbImage)>>,
}

impl Preview {
    fn invalidate(&mut self) {
        self.generation += 1;
    }

    fn update(&mut self, ctx: &Context, scene: &Scene, width: u32, height: u32) {
        if self.target != (width, height) {
            self.target = (width, height);
            self.invalidate();
        }
        if let Some(job) = &self.job {
            match job.try_recv() {
                Ok((generation, image)) => {
                    let picture = ColorImage::from_rgb(
                        [image.width() as usize, image.height() as usize],
                        image.as_raw(),
                    );
                    if let Some(texture) = &mut self.texture {
                        texture.set(picture, TextureOptions::LINEAR);
                    } else {
                        self.texture =
                            Some(ctx.load_texture("preview", picture, TextureOptions::LINEAR));
                    }
                    self.drawn = Some(generation);
                    self.job = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.drawn = Some(self.generation);
                    self.job = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.job.is_none() && self.drawn != Some(self.generation) {
            let scene = scene.clone();
            let generation = self.generation;
            let wake = ctx.clone();
            let (send, receive) = mpsc::channel();
            self.job = Some(receive);
            std::thread::spawn(move || {
                let result = render::render(&scene, width, height, false);
                let _ = send.send((generation, result));
                wake.request_repaint();
            });
        }
    }
}

fn fit(area: Rect, aspect: f32) -> Rect {
    let width = area.width().min(area.height() * aspect);
    Rect::from_center_size(area.center(), Vec2::new(width, width / aspect))
}

fn paint(ui: &Ui, view: Rect, texture: &TextureHandle) {
    ui.painter().image(
        texture.id(),
        view,
        Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        Color32::WHITE,
    );
    ui.painter().rect_stroke(
        view,
        0.0,
        Stroke::new(1.0_f32, BORDER),
        egui::StrokeKind::Outside,
    );
}

fn axes(
    ui: &mut Ui,
    title: &str,
    values: &mut [f32; 3],
    range: std::ops::RangeInclusive<f32>,
    speed: f32,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            Vec2::new(58.0, 24.0),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.label(title);
            },
        );
        for (axis, value) in ["X", "Y", "Z"].into_iter().zip(values) {
            changed |= ui
                .add_sized(
                    [63.0, 24.0],
                    DragValue::new(value)
                        .range(range.clone())
                        .speed(speed)
                        .prefix(format!("{axis} "))
                        .fixed_decimals(if speed < 1.0 { 3 } else { 0 }),
                )
                .on_hover_text(format!("{title} {axis}"))
                .changed();
        }
    });
    changed
}

fn slider(ui: &mut Ui, value: &mut f32, range: std::ops::RangeInclusive<f32>, label: &str) -> bool {
    ui.add(Slider::new(value, range).text(label)).changed()
}

fn color(ui: &mut Ui, value: &mut [f32; 3], name: &str) -> bool {
    ui.horizontal(|ui| {
        let mut pixels = value.map(|component| (component * 255.0).round() as u8);
        let changed = ui.color_edit_button_srgb(&mut pixels).changed();
        if changed {
            *value = pixels.map(|component| f32::from(component) / 255.0);
        }
        ui.label(name);
        changed
    })
    .inner
}

fn material(ui: &mut Ui, material: &mut Material, finish: bool) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt("material_pattern")
        .selected_text(material.pattern.name())
        .show_ui(ui, |ui| {
            for pattern in Pattern::ALL {
                changed |= ui
                    .selectable_value(&mut material.pattern, pattern, pattern.name())
                    .changed();
            }
        });
    changed |= color(ui, &mut material.color, "Base color");
    if material.pattern != Pattern::Solid {
        changed |= color(ui, &mut material.accent, "Accent color");
        changed |= slider(ui, &mut material.density, 0.2..=64.0, "Pattern scale");
    }
    if finish {
        changed |= slider(ui, &mut material.reflectivity, 0.0..=1.0, "Reflection");
        changed |= slider(ui, &mut material.roughness, 0.0..=1.0, "Roughness");
        changed |= slider(ui, &mut material.metallic, 0.0..=1.0, "Metallic");
    }
    changed
}

fn sky(ui: &mut Ui, scene: &mut Scene) -> bool {
    let sky = &mut scene.atmosphere;
    ui.label(RichText::new("Atmosphere").size(16.0).strong());
    let mut changed = color(ui, &mut sky.zenith, "Sky color");
    changed |= color(ui, &mut sky.horizon, "Horizon color");
    changed |= slider(ui, &mut sky.clouds, 0.0..=1.0, "Clouds");
    changed |= slider(ui, &mut sky.haze, 0.0..=1.0, "Haze");
    changed |= slider(ui, &mut sky.warmth, 0.0..=1.0, "Sun warmth");
    changed |= slider(ui, &mut sky.sun_heading, -180.0..=180.0, "Sun direction");
    changed |= slider(ui, &mut sky.sun_elevation, 1.0..=89.0, "Sun elevation");
    ui.separator();
    ui.label(RichText::new("Reflective plane").size(16.0).strong());
    let water = &mut scene.water;
    changed |= ui.checkbox(&mut water.enabled, "Show plane").changed();
    if water.enabled {
        changed |= slider(ui, &mut water.level, 0.0..=1.0, "Height");
        changed |= slider(ui, &mut water.ripples, 0.0..=1.0, "Ripples");
        changed |= slider(ui, &mut water.mirror, 0.0..=1.0, "Mirror strength");
        changed |= material(ui, &mut water.material, false);
    }
    changed
}

fn camera(ui: &mut Ui, scene: &mut Scene) -> bool {
    let camera = &mut scene.camera;
    ui.label(RichText::new("Camera").size(16.0).strong());
    let mut changed = slider(ui, &mut camera.heading, -180.0..=180.0, "Heading");
    changed |= slider(ui, &mut camera.elevation, -89.99..=89.99, "Elevation");
    changed |= slider(ui, &mut camera.distance, 0.05..=32.0, "Distance");
    changed |= slider(ui, &mut camera.field_of_view, 1.0..=179.0, "Field of view");
    if let Some(lens) = &mut camera.lens {
        changed |= slider(ui, &mut lens.roll, -180.0..=180.0, "Roll");
        changed |= slider(ui, &mut lens.shift[0], -1.0..=1.0, "Horizontal shift");
        changed |= slider(ui, &mut lens.shift[1], -1.0..=1.0, "Vertical shift");
        changed |= slider(ui, &mut lens.scale[0], 0.005..=10.0, "Horizontal scale");
        changed |= slider(ui, &mut lens.scale[1], 0.005..=10.0, "Vertical scale");
    }
    ui.separator();
    ui.label(RichText::new("Point of interest").strong());
    changed |= slider(ui, &mut camera.target_x, -128.0..=128.0, "X position");
    changed |= slider(ui, &mut camera.target_height, -128.0..=128.0, "Height");
    changed |= slider(ui, &mut camera.target_z, -128.0..=128.0, "Z position");
    changed
}

#[derive(Clone, Copy)]
enum Dialog {
    OpenProject,
    OpenMesh,
    OpenPicture,
    OpenMap,
    OpenIso,
    OpenBryceScene,
    SaveProject,
    SaveMap,
    SaveRender,
}

fn choose_file(dialog: Dialog) -> Result<Option<PathBuf>> {
    let (title, filter, default) = match dialog {
        Dialog::OpenProject => ("Open project", "Scene files | *.terrain *.BRC *.brc", None),
        Dialog::OpenMesh => ("Import Wavefront OBJ", "Wavefront mesh | *.obj *.OBJ", None),
        Dialog::OpenPicture => ("Import an image panel", "Images | *.png *.jpg *.jpeg *.bmp *.tif *.tiff *.PNG *.JPG *.BMP *.TIF", None),
        Dialog::OpenMap => ("Import heightmap", "Images | *.tif *.tiff *.TIF *.TIFF *.png *.jpg *.jpeg *.bmp *.pgm *.ppm *.PNG *.JPG *.BMP", None),
        Dialog::OpenIso => ("Open Bryce 2 disc", "ISO disc images | *.iso *.ISO", None),
        Dialog::OpenBryceScene => ("Open Bryce 2 scene", "Bryce 2 scenes | *.brc *.BRC", None),
        Dialog::SaveProject => ("Save project", "Terrain Studio project | *.terrain", Some("scene.terrain")),
        Dialog::SaveMap => ("Save heightmap", "PNG images | *.png", Some("heightmap.png")),
        Dialog::SaveRender => ("Save rendered image", "PNG images | *.png", Some("render.png")),
    };
    let mut command = Command::new("zenity");
    command
        .arg("--file-selection")
        .arg("--title")
        .arg(title)
        .arg("--file-filter")
        .arg(filter)
        .arg("--file-filter")
        .arg("All files | *");
    if let Some(name) = default {
        command
            .arg("--save")
            .arg("--confirm-overwrite")
            .arg("--filename")
            .arg(name);
    }
    let output = command.output()?;
    if output.status.code() == Some(1) && output.stderr.is_empty() {
        return Ok(None);
    }
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if output.status.code() == Some(1)
            && !stderr.contains("cannot open display")
            && !stderr.contains("Unknown option")
        {
            return Ok(None);
        }
        return Err(format!("file dialog failed: {}", stderr.trim()).into());
    }
    let mut name = output.stdout;
    if name.last() == Some(&b'\n') {
        name.pop();
    }
    Ok((!name.is_empty()).then(|| PathBuf::from(OsString::from_vec(name))))
}

fn filename(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}
