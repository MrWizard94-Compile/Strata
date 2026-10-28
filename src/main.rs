#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod benchmark;
mod daylight;
mod gpu_meshes;
mod material_export;
mod materials;
mod player;
mod renderer;
mod save;
mod streaming;
mod streaming_trace;
mod upload;
mod world;

use daylight::DayClock;
use egui::{Align2, Color32, FontId, Pos2, Rect, RichText, Stroke, Vec2};
use player::{Motion, Player};
use renderer::{PresentOutcome, RenderCamera, Renderer};
use save::SaveStore;
use std::{
    collections::{HashSet, VecDeque},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use streaming::{Streaming, UploadBudget, UploadStats};
use streaming_trace::{StreamingTrace, TraceFrame, TraceHeader};
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{DeviceEvent, ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{CursorGrabMode, Window, WindowId},
};
use world::{Block, World};

const ACCENT: Color32 = Color32::from_rgb(225, 184, 111);
const IVORY: Color32 = Color32::from_rgb(242, 238, 225);
const MUTED: Color32 = Color32::from_rgb(160, 178, 177);
const PANEL: Color32 = Color32::from_rgba_premultiplied(12, 24, 28, 236);
const HOTBAR: [Block; 8] = [
    Block::Grass,
    Block::Stone,
    Block::Wood,
    Block::Leaves,
    Block::Sand,
    Block::Clay,
    Block::Crystal,
    Block::Basalt,
];

struct Options {
    seed: u64,
    save_dir: PathBuf,
    smoke_seconds: Option<f32>,
    benchmark_cpu: Option<PathBuf>,
    export_materials: Option<PathBuf>,
    trace_streaming: Option<PathBuf>,
}
impl Options {
    fn parse() -> Result<Self, String> {
        let mut result = Self {
            seed: 728_491,
            save_dir: PathBuf::from("saves/emerald-reach"),
            smoke_seconds: None,
            benchmark_cpu: None,
            export_materials: None,
            trace_streaming: None,
        };
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--seed" => {
                    result.seed = args
                        .next()
                        .ok_or("--seed needs a value")?
                        .parse()
                        .map_err(|_| "Invalid seed")?
                }
                "--save-dir" => {
                    result.save_dir = PathBuf::from(args.next().ok_or("--save-dir needs a path")?)
                }
                "--benchmark-cpu" => {
                    result.benchmark_cpu = Some(PathBuf::from(
                        args.next()
                            .ok_or("--benchmark-cpu needs a new report path")?,
                    ));
                }
                "--export-materials" => {
                    result.export_materials = Some(PathBuf::from(
                        args.next()
                            .ok_or("--export-materials needs a new directory")?,
                    ));
                }
                "--trace-streaming" => {
                    result.trace_streaming = Some(PathBuf::from(
                        args.next()
                            .ok_or("--trace-streaming needs a new trace path")?,
                    ));
                }
                "--smoke-seconds" => {
                    let seconds: f32 = args
                        .next()
                        .ok_or("--smoke-seconds needs a value")?
                        .parse()
                        .map_err(|_| "Invalid smoke duration")?;
                    if !seconds.is_finite() || !(1.0..=120.0).contains(&seconds) {
                        return Err("Smoke duration must be 1–120 seconds".into());
                    }
                    result.smoke_seconds = Some(seconds);
                }
                "--help" | "-h" => {
                    return Err(
                        "STRATA: --seed <u64> --save-dir <directory> --smoke-seconds <1..120> --trace-streaming <new-trace.jsonl>\nCPU-only benchmark: --benchmark-cpu <new-report.json> (fixed dataset, no window or saves)\nMaterial PNG export: --export-materials <new-directory>"
                            .into(),
                    );
                }
                _ => return Err(format!("Unknown option: {arg}")),
            }
        }
        if result.benchmark_cpu.is_some() && result.export_materials.is_some() {
            return Err("Choose either a benchmark or a material export per invocation".into());
        }
        if result.trace_streaming.is_some()
            && (result.benchmark_cpu.is_some() || result.export_materials.is_some())
        {
            return Err("Streaming traces require the native client".into());
        }
        Ok(result)
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Screen {
    Title,
    Playing,
    Paused,
    Settings,
}

struct Game {
    window: Arc<Window>,
    renderer: Renderer,
    egui: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
    world: Arc<World>,
    player: Player,
    day: DayClock,
    streaming: Streaming,
    streaming_trace: Option<StreamingTrace>,
    last_upload: UploadStats,
    saves: SaveStore,
    screen: Screen,
    settings_return: Screen,
    has_entered: bool,
    focused: bool,
    keys: HashSet<KeyCode>,
    selected: usize,
    debug: bool,
    radius: i32,
    sensitivity: f32,
    start: Instant,
    last_frame: Instant,
    last_present: Option<Instant>,
    next_redraw: Instant,
    last_save: Instant,
    last_report: Instant,
    accumulator: f32,
    frame_times: VecDeque<f32>,
    frame_count: u64,
    skipped_frames: u64,
    status: String,
    status_until: Instant,
    fatal: Option<String>,
    dirty: bool,
    smoke_seconds: Option<f32>,
    should_exit: bool,
}

impl Game {
    fn new(window: Arc<Window>, options: &Options) -> Result<Self, String> {
        let (saves, loaded) = SaveStore::open(&options.save_dir)?;
        let (world, player, day, has_save) = match loaded {
            Some(snapshot) => {
                let day = DayClock::from_fraction(snapshot.time_of_day())?;
                let world = snapshot.world();
                (world, Player::restore(snapshot.player), day, true)
            }
            None => {
                let world = World::new(options.seed);
                let player = Player::new(&world);
                (world, player, DayClock::morning(), false)
            }
        };
        let renderer = pollster::block_on(Renderer::new(Arc::clone(&window)))?;
        println!(
            "STRATA_BOOT version=0.1.0 seed={} adapter={} saved_world={has_save}",
            world.seed(),
            renderer.adapter_name
        );
        let egui = egui::Context::default();
        let mut style = (*egui.style_of(egui::Theme::Dark)).clone();
        style.visuals = egui::Visuals::dark();
        style.visuals.override_text_color = Some(IVORY);
        style.visuals.panel_fill = PANEL;
        style.visuals.window_fill = PANEL;
        style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(32, 49, 52);
        style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(53, 73, 72);
        style.visuals.selection.bg_fill = ACCENT;
        style.spacing.item_spacing = Vec2::new(12.0, 12.0);
        style.spacing.button_padding = Vec2::new(18.0, 14.0);
        egui.set_theme(egui::Theme::Dark);
        egui.set_style_of(egui::Theme::Dark, style);
        let egui_state = egui_winit::State::new(
            egui.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            None,
            Some(renderer.device.limits().max_texture_dimension_2d as usize),
        );
        let egui_renderer = egui_wgpu::Renderer::new(
            &renderer.device,
            renderer.format,
            egui_wgpu::RendererOptions::default(),
        );
        let now = Instant::now();
        let streaming = Streaming::new();
        let budget = UploadBudget::default();
        let streaming_trace = options.trace_streaming.as_ref().map(|path| {
            StreamingTrace::create(path, &TraceHeader {
                kind: "header",
                schema_version: 2,
                package_version: env!("CARGO_PKG_VERSION"),
                seed: world.seed(),
                generator_version: world::GENERATOR_VERSION,
                initial_time_of_day: day.fraction(),
                adapter: &renderer.adapter_name,
                physical_size: [renderer.config.width, renderer.config.height],
                sample_count: renderer.sample_count(),
                present_mode: format!("{:?}", renderer.config.present_mode),
                workers: streaming.workers,
                upload_max_meshes: budget.max_meshes,
                upload_max_bytes: budget.max_bytes,
                upload_max_step_bytes: budget.max_step_bytes,
                upload_max_segments: streaming::MAX_COPY_SEGMENTS,
                upload_max_cpu_ms: budget.max_cpu_time.as_secs_f64() * 1000.0,
                source_label_external: std::env::var("STRATA_SOURCE_REVISION").ok(),
                measurement: "CPU-observed admission/upload callback/queue waits and presentation submission intervals. No GPU timestamps, display timing or total RAM/VRAM. Buffered trace I/O is observer overhead. Mesh copy payload is capped per redraw and per segment. Allocation/callback time remains non-preemptible; time is a soft limit.",
            })
        }).transpose()?;
        Ok(Self {
            window,
            renderer,
            egui,
            egui_state,
            egui_renderer,
            world: Arc::new(world),
            player,
            day,
            streaming,
            streaming_trace,
            last_upload: UploadStats::default(),
            saves,
            screen: Screen::Title,
            settings_return: Screen::Title,
            has_entered: false,
            focused: true,
            keys: HashSet::new(),
            selected: 0,
            debug: false,
            radius: 4,
            sensitivity: 1.0,
            start: now,
            last_frame: now,
            last_present: None,
            next_redraw: now,
            last_save: now,
            last_report: now,
            accumulator: 0.0,
            frame_times: VecDeque::with_capacity(240),
            frame_count: 0,
            skipped_frames: 0,
            status: if has_save {
                "Your world is ready to return to.".into()
            } else {
                "A new world. A place to begin.".into()
            },
            status_until: now + Duration::from_secs(5),
            fatal: None,
            dirty: false,
            smoke_seconds: options.smoke_seconds,
            should_exit: false,
        })
    }

    fn notify(&mut self, text: impl Into<String>) {
        self.status = text.into();
        self.status_until = Instant::now() + Duration::from_secs(4);
    }

    fn capture_cursor(&mut self, capture: bool) {
        if capture {
            if self
                .window
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| self.window.set_cursor_grab(CursorGrabMode::Confined))
                .is_err()
            {
                self.notify("Mouse capture unavailable. Arrow keys also turn the camera.");
            }
        } else {
            let _ = self.window.set_cursor_grab(CursorGrabMode::None);
        }
        self.window.set_cursor_visible(!capture);
    }

    fn enter(&mut self) {
        self.screen = Screen::Playing;
        self.has_entered = true;
        self.keys.clear();
        self.last_frame = Instant::now();
        self.last_present = None;
        self.frame_times.clear();
        self.next_redraw = Instant::now();
        self.accumulator = 0.0;
        self.capture_cursor(true);
        self.notify("WASD to explore · mouse to look · Esc to pause");
        println!("STRATA_ENTER eye={:?}", self.player.eye.to_array());
    }

    fn pause(&mut self) {
        self.screen = Screen::Paused;
        self.keys.clear();
        self.capture_cursor(false);
    }

    fn save(&mut self) -> bool {
        match self
            .saves
            .save(&self.world, self.player.snapshot(), self.day.fraction())
        {
            Ok(()) => {
                self.dirty = false;
                self.last_save = Instant::now();
                self.notify("World saved.");
                println!("STRATA_SAVE edits={}", self.world.edits().len());
                true
            }
            Err(error) => {
                self.notify(format!("Save failed: {error}"));
                eprintln!("STRATA_SAVE_FAILED {error}");
                false
            }
        }
    }

    fn edit(&mut self, place: bool) {
        if self.screen != Screen::Playing {
            return;
        }
        let Some(hit) = self.world.raycast(
            self.player.eye.to_array(),
            self.player.direction().to_array(),
            8.0,
        ) else {
            return;
        };
        let position = if place { hit.previous } else { hit.block };
        if place && self.player.occupies(position) {
            self.notify("Step back to place a block here.");
            return;
        }
        if !(-30..159).contains(&position[1]) {
            self.notify("This build supports edits between -30 and 158 blocks high.");
            return;
        }
        if self.world.edits().len() >= 99_999 {
            self.notify("This world's current edit limit has been reached.");
            return;
        }
        let block = if place {
            HOTBAR[self.selected]
        } else {
            Block::Air
        };
        Arc::make_mut(&mut self.world).set_block(position, block);
        self.streaming.invalidate_edit(position);
        self.dirty = true;
        println!(
            "STRATA_EDIT action={} position={position:?} material={}",
            if place { "place" } else { "break" },
            block.name()
        );
    }

    fn keyboard(&mut self, code: KeyCode, pressed: bool, repeat: bool) {
        if !pressed {
            self.keys.remove(&code);
            return;
        }
        self.keys.insert(code);
        if repeat {
            return;
        }
        match code {
            KeyCode::Escape => match self.screen {
                Screen::Playing => self.pause(),
                Screen::Paused => self.enter(),
                Screen::Settings => self.screen = self.settings_return,
                Screen::Title => {}
            },
            KeyCode::Enter if self.screen == Screen::Title || self.screen == Screen::Paused => {
                self.enter()
            }
            KeyCode::F3 => self.debug = !self.debug,
            KeyCode::F4
                if apply_debug_hour_skip(
                    self.debug,
                    &mut self.day,
                    self.has_entered,
                    &mut self.dirty,
                ) =>
            {
                self.notify(self.day.label());
            }
            KeyCode::F5 => {
                self.save();
            }
            KeyCode::KeyF if self.screen == Screen::Playing => {
                let changed = self.player.toggle_flight(&self.world);
                self.notify(if !changed {
                    "Move clear of the terrain before leaving flight."
                } else if self.player.flying {
                    "Flight enabled · Space up · Ctrl down"
                } else {
                    "Walking · Space to jump"
                });
            }
            KeyCode::KeyX => self.edit(false),
            KeyCode::KeyC => self.edit(true),
            // Arrow keys stay in `self.keys`. Turning is applied once per
            // simulation tick, so holding a key does not depend on OS repeat.
            KeyCode::Digit1 => self.selected = 0,
            KeyCode::Digit2 => self.selected = 1,
            KeyCode::Digit3 => self.selected = 2,
            KeyCode::Digit4 => self.selected = 3,
            KeyCode::Digit5 => self.selected = 4,
            KeyCode::Digit6 => self.selected = 5,
            KeyCode::Digit7 => self.selected = 6,
            KeyCode::Digit8 => self.selected = 7,
            _ => {}
        }
    }

    fn motion(&self) -> Motion {
        let key = |k| if self.keys.contains(&k) { 1.0 } else { 0.0 };
        Motion {
            forward: key(KeyCode::KeyW) - key(KeyCode::KeyS),
            right: key(KeyCode::KeyD) - key(KeyCode::KeyA),
            vertical: key(KeyCode::Space) - key(KeyCode::ControlLeft),
            sprint: self.keys.contains(&KeyCode::ShiftLeft),
            jump: self.keys.contains(&KeyCode::Space),
        }
    }

    fn camera(&self) -> RenderCamera {
        let mut eye = self.player.eye;
        let mut direction = self.player.direction();
        if !self.has_entered {
            eye += glam::Vec3::new(15.0, 28.0, 24.0);
            direction = glam::Vec3::new(-0.3, -0.3, -1.0).normalize();
        }
        RenderCamera {
            eye: eye.to_array(),
            target: (eye + direction).to_array(),
            time: self.start.elapsed().as_secs_f32(),
            fog_distance: self.radius as f32 * 32.0 - 12.0,
            daylight: self.day.sample(),
            selected: if self.screen == Screen::Playing {
                self.world
                    .raycast(
                        self.player.eye.to_array(),
                        self.player.direction().to_array(),
                        8.0,
                    )
                    .map(|h| h.block)
            } else {
                None
            },
        }
    }

    fn frame_stats(&self) -> (f32, f32) {
        if self.frame_times.is_empty() {
            return (0.0, 0.0);
        }
        let average = self.frame_times.iter().sum::<f32>() / self.frame_times.len() as f32;
        let mut sorted: Vec<_> = self.frame_times.iter().copied().collect();
        sorted.sort_by(f32::total_cmp);
        (
            1000.0 / average.max(0.001),
            sorted[(sorted.len() * 95 / 100).min(sorted.len() - 1)],
        )
    }

    fn redraw(&mut self) {
        if self.window.inner_size().width == 0 || self.window.inner_size().height == 0 {
            return;
        }
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;
        let (steps, accumulator) =
            simulation_steps(self.screen, self.focused, self.accumulator, elapsed);
        self.accumulator = accumulator;
        if steps > 0 {
            let motion = self.motion();
            for _ in 0..steps {
                let (yaw, pitch) = arrow_look_delta(&self.keys, self.screen);
                if yaw != 0.0 || pitch != 0.0 {
                    self.player.look(yaw, pitch);
                }
                self.player
                    .tick(&self.world, motion, daylight::SIM_TICK_SECONDS);
                self.day.advance(false);
            }
        }
        let camera = self.camera();
        // UI actions below can change these for the next redraw. Keep each
        // trace row aligned with this camera and this admission workload.
        let trace_screen = match self.screen {
            Screen::Title => "title",
            Screen::Playing => "playing",
            Screen::Paused => "paused",
            Screen::Settings => "settings",
        };
        let trace_radius = self.radius;
        let trace_time_of_day = self.day.fraction();
        for key in self.streaming.update(camera.eye, self.radius, &self.world) {
            self.renderer.remove_chunk(key);
        }
        match self
            .streaming
            .upload_ready(UploadBudget::default(), |command| {
                self.renderer.upload(command)
            }) {
            Ok(stats) => self.last_upload = stats,
            Err(error) => {
                self.fatal = Some(format!("Mesh upload failed: {error}"));
                self.should_exit = true;
                self.save();
                return;
            }
        }
        if self.has_entered && now.duration_since(self.last_save) > Duration::from_secs(30) {
            self.last_save = now;
            self.save();
        }
        if now.duration_since(self.last_report) > Duration::from_secs(10) {
            self.last_report = now;
            let (fps, p95) = self.frame_stats();
            println!(
                "STRATA_FRAME presented={} skipped={} present_fps={fps:.1} present_p95_ms={p95:.2} chunks={} triangles={} mesh_ms={:.2}",
                self.frame_count,
                self.skipped_frames,
                self.renderer.chunk_count(),
                self.renderer.triangle_count(),
                self.streaming.last_mesh_ms
            );
        }
        let input = self.egui_state.take_egui_input(&self.window);
        let context = self.egui.clone();
        let output = context.run_pass(input, |ctx| self.draw_ui(ctx));
        self.egui_state
            .handle_platform_output(&self.window, output.platform_output);
        if self.screen == Screen::Playing {
            self.window.set_cursor_visible(false);
        }
        let paint_jobs = context.tessellate(output.shapes, output.pixels_per_point);
        let mut present_interval_ms = None;
        let outcome;
        let trace_eye = camera.eye;
        match self.renderer.render(
            camera,
            &mut self.egui_renderer,
            &paint_jobs,
            &output.textures_delta,
            output.pixels_per_point,
        ) {
            Ok(PresentOutcome::Presented) => {
                outcome = "presented";
                let presented_at = Instant::now();
                if let Some(previous) = self.last_present.replace(presented_at) {
                    present_interval_ms =
                        Some(presented_at.duration_since(previous).as_secs_f64() * 1000.0);
                    self.frame_times
                        .push_back(presented_at.duration_since(previous).as_secs_f32() * 1000.0);
                    if self.frame_times.len() > 240 {
                        self.frame_times.pop_front();
                    }
                }
                self.frame_count += 1;
                self.next_redraw = presented_at
                    + if self.focused {
                        Duration::ZERO
                    } else {
                        Duration::from_millis(50)
                    };
            }
            Ok(PresentOutcome::Skipped) => {
                outcome = "skipped";
                self.skipped_frames += 1;
                self.next_redraw = Instant::now() + Duration::from_millis(50);
            }
            Err(error) => {
                outcome = "failed";
                self.fatal = Some(format!("Rendering stopped: {error}"));
                eprintln!("STRATA_RENDER_FAILED {error}");
                self.should_exit = true;
            }
        }
        if let Some(trace) = &mut self.streaming_trace {
            let (current_chunks, desired_chunks) = self.streaming.progress();
            let record = TraceFrame {
                kind: "frame",
                elapsed_ms: self.start.elapsed().as_secs_f64() * 1000.0,
                redraw_interval_ms: f64::from(elapsed) * 1000.0,
                present_interval_ms,
                outcome,
                screen: trace_screen,
                focused: self.focused,
                eye: trace_eye,
                physical_size: [self.renderer.config.width, self.renderer.config.height],
                radius_chunks: trace_radius,
                presented: self.frame_count,
                skipped: self.skipped_frames,
                current_chunks,
                desired_chunks,
                resident_meshes: self.renderer.chunk_count(),
                resident_mesh_payload_bytes: self.renderer.resident_mesh_payload_bytes(),
                resident_triangles: self.renderer.triangle_count(),
                uploads: self.last_upload,
                time_of_day: trace_time_of_day,
            };
            if let Err(error) = trace.frame(&record) {
                eprintln!("STRATA_TRACE_FAILED {error}");
                self.streaming_trace = None;
                self.fatal = Some(format!("Streaming trace failed: {error}"));
                self.should_exit = true;
            }
        }
        if self
            .smoke_seconds
            .is_some_and(|s| self.start.elapsed().as_secs_f32() >= s)
        {
            let (fps, p95) = self.frame_stats();
            println!(
                "STRATA_SMOKE_COMPLETE presented={} skipped={} chunks={} triangles={} present_fps={fps:.1} present_p95_ms={p95:.2}",
                self.frame_count,
                self.skipped_frames,
                self.renderer.chunk_count(),
                self.renderer.triangle_count()
            );
            self.should_exit = true;
        }
    }

    fn draw_ui(&mut self, ctx: &egui::Context) {
        let bounds = ctx.content_rect();
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Background,
            egui::Id::unique("world-overlay"),
        ));
        painter.text(
            Pos2::new(34.0, 30.0),
            Align2::LEFT_TOP,
            "S T R A T A",
            FontId::proportional(26.0),
            IVORY,
        );
        painter.text(
            Pos2::new(36.0, 65.0),
            Align2::LEFT_TOP,
            "T H E   E M E R A L D   R E A C H",
            FontId::proportional(10.0),
            ACCENT,
        );
        painter.text(
            Pos2::new(bounds.right() - 28.0, 30.0),
            Align2::RIGHT_TOP,
            "DEVELOPMENT BUILD   /   0.1",
            FontId::monospace(10.0),
            IVORY,
        );
        if self.screen != Screen::Playing {
            painter.rect_filled(bounds, 0.0, Color32::from_black_alpha(65));
            let position = Pos2::new(56.0, (bounds.height() * 0.26).max(110.0));
            egui::Area::new(egui::Id::unique("menu")).fixed_pos(position).show(ctx, |ui| {
                egui::Frame::new().fill(PANEL).inner_margin(32.0).corner_radius(4).show(ui, |ui| {
                    ui.set_width(350.0);
                    match self.screen {
                        Screen::Title => {
                            ui.label(RichText::new("01  /  BEGIN AN EXPEDITION").color(ACCENT).size(11.0));
                            ui.add_space(14.0);
                            ui.label(RichText::new("A world\nto make your own.").size(38.0).strong());
                            ui.add_space(14.0);
                            ui.label(RichText::new("Walk the ridgelines. Carve into the earth.\nBuild something that belongs here.").size(15.0).color(MUTED));
                            ui.add_space(24.0);
                            if primary_button(ui,"ENTER THE REACH") { self.enter(); }
                            if secondary_button(ui,"World settings") { self.settings_return=Screen::Title; self.screen=Screen::Settings; }
                            if secondary_button(ui,"Quit") { self.should_exit=true; }
                            ui.add_space(12.0);
                            ui.label(RichText::new(format!("WORLD SEED  {:06}",self.world.seed())).monospace().size(10.0).color(MUTED));
                        }
                        Screen::Paused => {
                            ui.label(RichText::new("TAKE A BREATH").color(ACCENT).size(11.0));
                            ui.add_space(10.0);
                            ui.label(RichText::new("The world can wait.").size(30.0).strong());
                            ui.add_space(20.0);
                            if primary_button(ui,"CONTINUE EXPLORING") { self.enter(); }
                            if secondary_button(ui,"Save world") { self.save(); }
                            if secondary_button(ui,"Settings") { self.settings_return=Screen::Paused; self.screen=Screen::Settings; }
                            if secondary_button(ui,"Save and quit") && self.save() { self.should_exit=true; }
                            ui.add_space(10.0);
                            ui.label(RichText::new("WASD  Move     Space  Jump     Shift  Sprint\nF  Flight     1–8  Material     F5  Save\nLeft click / X  Break     Right click / C  Place\nArrows  Turn     F3  Performance     Esc  Pause").size(12.0).color(MUTED));
                        }
                        Screen::Settings => {
                            ui.label(RichText::new("MAKE YOURSELF AT HOME").color(ACCENT).size(11.0));
                            ui.add_space(10.0);
                            ui.heading("World settings"); ui.add_space(18.0);
                            ui.label("View distance");
                            ui.add(egui::Slider::new(&mut self.radius,2..=6).suffix(" chunks"));
                            ui.label(RichText::new("Each chunk is 32 blocks wide. Higher distances use more memory and take longer to stream.").color(MUTED).size(12.0));
                            ui.add_space(12.0);
                            ui.label("Mouse sensitivity");
                            ui.add(egui::Slider::new(&mut self.sensitivity,0.3..=2.5));
                            ui.checkbox(&mut self.debug,"Show performance overlay");
                            ui.add_space(14.0);
                            if primary_button(ui,"BACK") { self.screen=self.settings_return; }
                        }
                        Screen::Playing => {}
                    }
                });
            });
        } else {
            let c = bounds.center();
            for (a, b) in [
                (Vec2::new(-9.0, 0.0), Vec2::new(-3.0, 0.0)),
                (Vec2::new(3.0, 0.0), Vec2::new(9.0, 0.0)),
                (Vec2::new(0.0, -9.0), Vec2::new(0.0, -3.0)),
                (Vec2::new(0.0, 3.0), Vec2::new(0.0, 9.0)),
            ] {
                painter.line_segment([c + a, c + b], Stroke::new(1.6, IVORY));
            }
            let slot = 56.0;
            let total = slot * 8.0;
            let start = Pos2::new(bounds.center().x - total / 2.0, bounds.bottom() - 86.0);
            for (i, block) in HOTBAR.iter().enumerate() {
                let rect = Rect::from_min_size(
                    start + Vec2::new(i as f32 * slot, 0.0),
                    Vec2::splat(slot - 4.0),
                );
                painter.rect_filled(
                    rect,
                    4.0,
                    if i == self.selected {
                        Color32::from_rgba_premultiplied(66, 68, 51, 235)
                    } else {
                        PANEL
                    },
                );
                if i == self.selected {
                    painter.rect_stroke(
                        rect,
                        4.0,
                        Stroke::new(1.5, ACCENT),
                        egui::StrokeKind::Inside,
                    );
                }
                let color = block.color();
                let rgb = Color32::from_rgb(
                    (color[0] * 255.0) as u8,
                    (color[1] * 255.0) as u8,
                    (color[2] * 255.0) as u8,
                );
                let icon =
                    Rect::from_center_size(rect.center() + Vec2::new(0.0, -4.0), Vec2::splat(23.0));
                painter.rect_filled(icon, 3.0, rgb);
                painter.line_segment(
                    [icon.left_top(), icon.right_top()],
                    Stroke::new(3.0, Color32::from_white_alpha(65)),
                );
                painter.text(
                    rect.center_bottom() + Vec2::new(0.0, -3.0),
                    Align2::CENTER_BOTTOM,
                    (i + 1).to_string(),
                    FontId::monospace(9.0),
                    MUTED,
                );
            }
            painter.text(
                Pos2::new(bounds.center().x, bounds.bottom() - 101.0),
                Align2::CENTER_BOTTOM,
                HOTBAR[self.selected].name(),
                FontId::proportional(14.0),
                IVORY,
            );
            painter.text(
                Pos2::new(bounds.center().x, bounds.bottom() - 17.0),
                Align2::CENTER_BOTTOM,
                "BREAK  LMB    /    PLACE  RMB    /    FLIGHT  F    /    PAUSE  ESC",
                FontId::monospace(10.0),
                IVORY,
            );
            painter.text(
                Pos2::new(36.0, 96.0),
                Align2::LEFT_TOP,
                if self.player.flying {
                    "FREE FLIGHT"
                } else {
                    "EXPLORING"
                },
                FontId::monospace(10.0),
                IVORY,
            );
        }
        let (loaded, total) = self.streaming.progress();
        if loaded < total {
            egui::Area::new(egui::Id::unique("streaming-progress"))
                .anchor(Align2::RIGHT_BOTTOM, Vec2::new(-28.0, -28.0))
                .show(ctx, |ui| {
                    egui::Frame::new()
                        .fill(PANEL)
                        .inner_margin(12.0)
                        .corner_radius(4)
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new("THE WORLD IS UNFOLDING")
                                    .size(10.0)
                                    .color(ACCENT),
                            );
                            ui.add(
                                egui::ProgressBar::new(loaded as f32 / total.max(1) as f32)
                                    .desired_width(180.0)
                                    .fill(ACCENT),
                            );
                        });
                });
        }
        if Instant::now() < self.status_until {
            let notification = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::unique("notification"),
            ));
            let rect = Rect::from_center_size(
                Pos2::new(bounds.center().x, 116.0),
                Vec2::new(
                    (self.status.len() as f32 * 7.0 + 32.0).min(bounds.width() - 40.0),
                    34.0,
                ),
            );
            notification.rect_filled(rect, 4.0, PANEL);
            notification.text(
                rect.center(),
                Align2::CENTER_CENTER,
                &self.status,
                FontId::proportional(13.0),
                IVORY,
            );
        }
        if self.debug {
            let (fps, p95) = self.frame_stats();
            egui::Area::new(egui::Id::unique("performance")).anchor(Align2::RIGHT_TOP,Vec2::new(-28.0,64.0)).show(ctx,|ui| {
                egui::Frame::new().fill(PANEL).inner_margin(16.0).corner_radius(4).show(ui,|ui| {
                    ui.label(RichText::new("FIELD NOTES").color(ACCENT).size(11.0));
                    ui.monospace(format!(
                        "{fps:.0} fps  ·  p95 {p95:.2} ms\n{} chunks  ·  {} triangles\n{} workers  ·  {:.2} ms last mesh\nPosition {:.1}, {:.1}, {:.1}\n{}\nF4 +1 hour\n{}",
                        self.renderer.chunk_count(),
                        self.renderer.triangle_count(),
                        self.streaming.workers,
                        self.streaming.last_mesh_ms,
                        self.player.eye.x,
                        self.player.eye.y,
                        self.player.eye.z,
                        self.day.label(),
                        self.renderer.adapter_name
                    ));
                });
            });
        }
    }
}

fn primary_button(ui: &mut egui::Ui, label: &str) -> bool {
    ui.add_sized(
        [ui.available_width(), 48.0],
        egui::Button::new(
            RichText::new(label)
                .size(13.0)
                .strong()
                .color(Color32::from_rgb(24, 34, 34)),
        )
        .fill(ACCENT)
        .corner_radius(3),
    )
    .clicked()
}
fn secondary_button(ui: &mut egui::Ui, label: &str) -> bool {
    ui.add_sized(
        [ui.available_width(), 40.0],
        egui::Button::new(RichText::new(label).size(13.0)).corner_radius(3),
    )
    .clicked()
}

struct Application {
    options: Options,
    game: Option<Game>,
    error: Option<String>,
}
impl ApplicationHandler for Application {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.game.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("STRATA — The Emerald Reach")
            .with_inner_size(LogicalSize::new(1440.0, 900.0))
            .with_min_inner_size(LogicalSize::new(900.0, 600.0))
            .with_visible(false);
        match event_loop
            .create_window(attributes)
            .map_err(|e| e.to_string())
            .and_then(|window| Game::new(Arc::new(window), &self.options))
        {
            Ok(game) => {
                game.window.set_visible(true);
                game.window.request_redraw();
                self.game = Some(game);
            }
            Err(error) => {
                report_startup_failure(&error);
                self.error = Some(error);
                event_loop.exit();
            }
        }
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(game) = &mut self.game else {
            return;
        };
        if game.window.id() != id {
            return;
        }
        let response = game.egui_state.on_window_event(&game.window, &event);
        match event {
            WindowEvent::CloseRequested => {
                if !game.has_entered || game.save() {
                    game.should_exit = true;
                } else {
                    game.pause();
                }
            }
            WindowEvent::Resized(size) => game.renderer.resize(size.width, size.height),
            WindowEvent::Focused(focused) => {
                game.focused = focused;
                if unfocused_play_pauses(game.screen, focused) {
                    game.pause();
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key
                    && game_key_reaches_game(code, response.consumed)
                {
                    game.keyboard(code, event.state == ElementState::Pressed, event.repeat);
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } if !response.consumed && game.screen == Screen::Playing => match button {
                MouseButton::Left => game.edit(false),
                MouseButton::Right => game.edit(true),
                _ => {}
            },
            WindowEvent::MouseWheel { delta, .. } if game.screen == Screen::Playing => {
                let y = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32,
                };
                if y != 0.0 {
                    game.selected = (game.selected as i32 + if y > 0.0 { -1 } else { 1 })
                        .rem_euclid(8) as usize;
                }
            }
            WindowEvent::RedrawRequested => game.redraw(),
            _ => {}
        }
        if game.should_exit {
            if let Some(error) = game.fatal.take() {
                // A controlled renderer failure does not invalidate the CPU world.
                // Attempt preservation before teardown, retaining both diagnostics.
                if game.has_entered {
                    game.save();
                }
                self.error = Some(error);
            }
            event_loop.exit();
        }
    }
    fn device_event(&mut self, _: &ActiveEventLoop, _: winit::event::DeviceId, event: DeviceEvent) {
        if let Some(game) = &mut self.game
            && game.screen == Screen::Playing
            && game.focused
            && let DeviceEvent::MouseMotion { delta } = event
        {
            game.player.look(
                delta.0 as f32 * 0.002 * game.sensitivity,
                -delta.1 as f32 * 0.002 * game.sensitivity,
            );
        }
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(game) = &self.game {
            if game.window.is_minimized().unwrap_or(false) {
                event_loop.set_control_flow(ControlFlow::Wait);
            } else if Instant::now() < game.next_redraw {
                event_loop.set_control_flow(ControlFlow::WaitUntil(game.next_redraw));
            } else {
                event_loop.set_control_flow(ControlFlow::Wait);
                game.window.request_redraw();
            }
        }
    }
}

/// Steps the fixed clock may take this frame. Title, pause, settings, and
/// focus loss clear the accumulator so the day does not advance.
fn simulation_steps(
    screen: Screen,
    focused: bool,
    mut accumulator: f32,
    elapsed: f32,
) -> (u32, f32) {
    if !(screen == Screen::Playing && focused) {
        return (0, 0.0);
    }
    accumulator += elapsed.min(0.1);
    let mut steps = 0u32;
    while accumulator >= daylight::SIM_TICK_SECONDS {
        accumulator -= daylight::SIM_TICK_SECONDS;
        steps += 1;
    }
    (steps, accumulator)
}

fn unfocused_play_pauses(screen: Screen, focused: bool) -> bool {
    !focused && screen == Screen::Playing
}

/// Radians applied on one 60 Hz tick while an arrow key is held. Two radians
/// per second, so a held key keeps turning without OS key repeat.
const ARROW_LOOK_PER_TICK: f32 = 2.0 * daylight::SIM_TICK_SECONDS;

fn arrow_look_delta(keys: &HashSet<KeyCode>, screen: Screen) -> (f32, f32) {
    if screen != Screen::Playing {
        return (0.0, 0.0);
    }
    let axis = |negative: KeyCode, positive: KeyCode| {
        let mut value = 0.0;
        if keys.contains(&negative) {
            value -= ARROW_LOOK_PER_TICK;
        }
        if keys.contains(&positive) {
            value += ARROW_LOOK_PER_TICK;
        }
        value
    };
    (
        axis(KeyCode::ArrowLeft, KeyCode::ArrowRight),
        axis(KeyCode::ArrowDown, KeyCode::ArrowUp),
    )
}

/// Writes `STRATA_START_FAILED` beside the executable. A Windows GUI-subsystem
/// build has no console, so this file is the message the player can open.
fn record_startup_failure(directory: &std::path::Path, error: &str) -> std::io::Result<()> {
    std::fs::write(
        directory.join("strata-error.txt"),
        format!("STRATA_START_FAILED {error}\n"),
    )
}

fn report_startup_failure(error: &str) {
    eprintln!("STRATA_START_FAILED {error}");
    let directory = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    if let Err(write_error) = record_startup_failure(&directory, error) {
        eprintln!("STRATA_START_FAILED could not write strata-error.txt: {write_error}");
    }
}

fn game_key_reaches_game(code: KeyCode, egui_consumed: bool) -> bool {
    !egui_consumed
        || matches!(
            code,
            KeyCode::Escape | KeyCode::F3 | KeyCode::F4 | KeyCode::F5 | KeyCode::Enter
        )
}

fn apply_debug_hour_skip(
    debug: bool,
    day: &mut DayClock,
    has_entered: bool,
    dirty: &mut bool,
) -> bool {
    if !debug {
        return false;
    }
    day.skip_hours(1.0);
    if has_entered {
        *dirty = true;
    }
    true
}

fn main() {
    let options = match Options::parse() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    if let Some(output) = &options.benchmark_cpu {
        match benchmark::run_cpu_benchmark(output) {
            Ok(()) => println!("STRATA_CPU_BENCHMARK report={}", output.display()),
            Err(error) => {
                eprintln!("STRATA_CPU_BENCHMARK_FAILED {error}");
                std::process::exit(1);
            }
        }
        return;
    }
    if let Some(output) = &options.export_materials {
        if let Err(error) = material_export::export_materials(output) {
            eprintln!("STRATA_MATERIAL_EXPORT_FAILED {error}");
            std::process::exit(1);
        }
        println!("STRATA_MATERIAL_EXPORT directory={}", output.display());
        return;
    }
    let event_loop = match EventLoop::new() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Cannot create window event loop: {e}");
            std::process::exit(1);
        }
    };
    let mut app = Application {
        options,
        game: None,
        error: None,
    };
    if let Err(error) = event_loop.run_app(&mut app) {
        eprintln!("Application event loop failed: {error}");
        if let Some(game) = &mut app.game
            && game.has_entered
        {
            game.save();
        }
        app.error = Some(error.to_string());
    }
    if let Some(game) = &mut app.game
        && let Some(trace) = game.streaming_trace.take()
        && let Err(error) = trace.finish(app.error.is_none())
    {
        eprintln!("STRATA_TRACE_FINISH_FAILED {error}");
        app.error = Some(error);
    }
    if app.error.is_some() {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clock_after(screen: Screen, focused: bool, frames: u32) -> (DayClock, f32) {
        let mut clock = DayClock::morning();
        let mut accumulator = 0.0;
        for _ in 0..frames {
            let (steps, next) =
                simulation_steps(screen, focused, accumulator, daylight::SIM_TICK_SECONDS);
            accumulator = next;
            for _ in 0..steps {
                clock.advance(false);
            }
        }
        (clock, accumulator)
    }

    #[test]
    fn title_pause_settings_and_focus_loss_freeze_the_clock() {
        let morning = DayClock::morning().fraction();
        let (cleared, cleared_accumulator) =
            simulation_steps(Screen::Paused, true, 0.04, daylight::SIM_TICK_SECONDS);
        assert_eq!(cleared, 0);
        assert_eq!(cleared_accumulator, 0.0);
        for screen in [Screen::Title, Screen::Paused, Screen::Settings] {
            for focused in [true, false] {
                let (clock, accumulator) = clock_after(screen, focused, 180);
                assert_eq!(clock.fraction(), morning);
                assert_eq!(accumulator, 0.0);
            }
        }
        let (playing, _) = clock_after(Screen::Playing, true, 120);
        assert_eq!(playing.tick(), DayClock::morning().tick() + 120);

        let mut screen = Screen::Playing;
        let focused = false;
        if unfocused_play_pauses(screen, focused) {
            screen = Screen::Paused;
        }
        assert!(screen == Screen::Paused);
        let (clock, accumulator) = clock_after(screen, focused, 90);
        assert_eq!(clock.fraction(), morning);
        assert_eq!(accumulator, 0.0);
        assert!(!unfocused_play_pauses(Screen::Paused, false));
        assert!(!unfocused_play_pauses(Screen::Title, false));
        assert!(!unfocused_play_pauses(Screen::Settings, false));
        assert!(!unfocused_play_pauses(Screen::Playing, true));
    }

    #[test]
    fn three_days_of_focused_play_wrap_the_clock() {
        let start = DayClock::from_fraction(0.0).unwrap();
        let mut clock = start;
        let mut accumulator = 0.0;
        let frames = daylight::TICKS_PER_DAY * 3;
        for _ in 0..frames {
            let (steps, next) = simulation_steps(
                Screen::Playing,
                true,
                accumulator,
                daylight::SIM_TICK_SECONDS,
            );
            assert_eq!(steps, 1);
            accumulator = next;
            clock.advance(false);
        }
        assert_eq!(clock.tick(), start.tick());
        assert!(accumulator.abs() < 1e-4);
        let (burst, left) = simulation_steps(Screen::Playing, true, 0.0, 10.0);
        let capped = 0.1_f32;
        assert!((left + burst as f32 * daylight::SIM_TICK_SECONDS - capped).abs() < 1e-5);
        assert!(left < daylight::SIM_TICK_SECONDS);
        assert!(burst < 60);
    }

    #[test]
    fn f4_reaches_the_game_and_skips_one_hour_only_while_debugging() {
        assert!(game_key_reaches_game(KeyCode::F4, true));
        assert!(game_key_reaches_game(KeyCode::F4, false));
        assert!(!game_key_reaches_game(KeyCode::KeyW, true));
        assert!(game_key_reaches_game(KeyCode::Escape, true));

        let mut day = DayClock::morning();
        let before = day.fraction();
        let mut dirty = false;
        assert!(!apply_debug_hour_skip(false, &mut day, true, &mut dirty));
        assert_eq!(day.fraction(), before);
        assert!(!dirty);

        assert!(apply_debug_hour_skip(true, &mut day, false, &mut dirty));
        assert_eq!(day.label(), "09:00 morning");
        assert!(!dirty);
        assert!(apply_debug_hour_skip(true, &mut day, true, &mut dirty));
        assert_eq!(day.label(), "10:00 morning");
        assert!(dirty);
    }

    #[test]
    fn held_arrow_turns_on_each_simulation_tick() {
        let mut keys = HashSet::new();
        assert_eq!(arrow_look_delta(&keys, Screen::Playing), (0.0, 0.0));
        keys.insert(KeyCode::ArrowRight);
        keys.insert(KeyCode::ArrowUp);
        assert_eq!(
            arrow_look_delta(&keys, Screen::Paused),
            (0.0, 0.0),
            "pause and title do not turn"
        );
        let (yaw_step, pitch_step) = arrow_look_delta(&keys, Screen::Playing);
        assert_eq!(yaw_step, ARROW_LOOK_PER_TICK);
        assert_eq!(pitch_step, ARROW_LOOK_PER_TICK);
        keys.insert(KeyCode::ArrowLeft);
        keys.insert(KeyCode::ArrowDown);
        assert_eq!(
            arrow_look_delta(&keys, Screen::Playing),
            (0.0, 0.0),
            "opposite arrows cancel"
        );

        let mut player = Player::restore(save::PlayerSave {
            eye: [0.0, 10.0, 0.0],
            yaw: 0.0,
            pitch: 0.0,
            flying: false,
        });
        let held = HashSet::from([KeyCode::ArrowRight]);
        let ticks = daylight::SIM_TICKS_PER_SECOND;
        for _ in 0..ticks {
            let (yaw, pitch) = arrow_look_delta(&held, Screen::Playing);
            player.look(yaw, pitch);
        }
        assert!((player.yaw - 2.0).abs() < 1e-4);
        assert_eq!(player.pitch, 0.0);
    }

    #[test]
    fn startup_failure_is_written_beside_the_chosen_directory() {
        let dir = std::env::temp_dir().join(format!(
            "strata-start-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        record_startup_failure(&dir, "no Vulkan or DirectX 12 adapter").unwrap();
        let text = std::fs::read_to_string(dir.join("strata-error.txt")).unwrap();
        assert_eq!(
            text,
            "STRATA_START_FAILED no Vulkan or DirectX 12 adapter\n"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
