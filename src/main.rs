use chrono::Datelike;
use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};
use rusqlite::{Connection, params};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant, SystemTime},
};
use sysinfo::{Components, Disks, Gpus, Networks, System, Users};

#[derive(Clone, Copy)]
struct Palette {
    primary: Color32,
    secondary: Color32,
    tertiary: Color32,
}

impl Palette {
    const FALLBACK: Self = Self {
        primary: Color32::from_rgb(29, 32, 33),
        secondary: Color32::from_rgb(176, 162, 123),
        tertiary: Color32::from_rgb(118, 173, 188),
    };
}

thread_local! {
    static ACTIVE_PALETTE: std::cell::Cell<Palette> = const { std::cell::Cell::new(Palette::FALLBACK) };
}

fn palette() -> Palette {
    ACTIVE_PALETTE.with(std::cell::Cell::get)
}
fn primary() -> Color32 {
    palette().primary
}
fn secondary() -> Color32 {
    palette().secondary
}
fn tertiary() -> Color32 {
    palette().tertiary
}
fn with_alpha(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}
fn glass() -> Color32 {
    with_alpha(primary(), 174)
}
fn glass_strong() -> Color32 {
    with_alpha(primary(), 222)
}
fn glass_border() -> Color32 {
    with_alpha(secondary(), 82)
}
fn muted() -> Color32 {
    with_alpha(secondary(), 178)
}
fn teal() -> Color32 {
    secondary()
}
fn blue() -> Color32 {
    secondary()
}
fn purple() -> Color32 {
    secondary()
}

fn install_palette(next: Palette) {
    ACTIVE_PALETTE.with(|active| active.set(next));
}

fn system_scheme_path() -> Option<std::path::PathBuf> {
    let config = dirs::config_dir()?.join("hypr");
    let lua_config = config.join("hyprland.lua").is_file();
    let extension = if lua_config { "lua" } else { "conf" };
    Some(config.join("scheme").join(format!("current.{extension}")))
}

fn parse_system_palette(path: &std::path::Path) -> Option<Palette> {
    let contents = std::fs::read_to_string(path).ok()?;
    let mut colors = HashMap::<&str, Color32>::new();
    for line in contents.lines() {
        let line = line.trim();
        let Some((key, value)) = line.strip_prefix('$').unwrap_or(line).split_once('=') else {
            continue;
        };
        let value = value
            .trim()
            .trim_end_matches(',')
            .trim()
            .trim_matches('"')
            .trim_start_matches('#');
        if value.len() != 6 {
            continue;
        }
        let Ok(rgb) = u32::from_str_radix(value, 16) else {
            continue;
        };
        colors.insert(
            key.trim(),
            Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8),
        );
    }
    Some(Palette {
        primary: *colors.get("surface")?,
        secondary: *colors.get("onSurface")?,
        tertiary: *colors.get("primary")?,
    })
}

fn apply_egui_palette(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    let accent = tertiary();
    let foreground = secondary();
    let border = glass_border();
    visuals.override_text_color = Some(foreground);
    visuals.weak_text_color = Some(muted());
    visuals.hyperlink_color = accent;
    visuals.selection.bg_fill = with_alpha(accent, 70);
    visuals.selection.stroke.color = foreground;
    visuals.panel_fill = primary();
    visuals.window_fill = glass_strong();
    visuals.window_stroke.color = border;
    visuals.extreme_bg_color = primary();
    visuals.faint_bg_color = with_alpha(secondary(), 20);
    visuals.code_bg_color = glass();
    visuals.text_edit_bg_color = Some(glass_strong());
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.fg_stroke.color = foreground;
        widget.bg_stroke.color = border;
    }
    visuals.widgets.hovered.bg_fill = with_alpha(accent, 50);
    visuals.widgets.active.bg_fill = with_alpha(accent, 86);
    ctx.set_visuals(visuals);
}

#[derive(Clone, Copy, PartialEq)]
enum Page {
    General,
    Cpu,
    Memory,
    Network,
    Processes,
}
#[derive(Clone, Copy, PartialEq)]
enum ProcessSort {
    Cpu,
    Memory,
    Pid,
    Name,
    Arguments,
    Threads,
    Owner,
}
#[derive(Clone)]
struct ProcessRow {
    pid: u32,
    name: String,
    executable: String,
    command: String,
    owner: String,
    parent: Option<u32>,
    threads: usize,
    cpu: f32,
    memory: u64,
    status: String,
    read: u64,
    written: u64,
    runtime: u64,
}
#[derive(Clone)]
struct GpuSample {
    name: String,
    usage: Option<f32>,
    used_memory: Option<u64>,
    total_memory: Option<u64>,
}
struct Sample {
    cpu: f32,
    ram_used: f64,
    ram_total: f64,
    down: u64,
    up: u64,
}
struct App {
    palette_path: Option<std::path::PathBuf>,
    palette_modified: Option<SystemTime>,
    last_palette_check: Instant,
    system: System,
    networks: Networks,
    disks: Disks,
    gpus: Gpus,
    components: Components,
    users: Users,
    db: Connection,
    sample: Sample,
    cpu_history: VecDeque<f32>,
    memory_history: VecDeque<(f32, f32)>,
    history: Vec<(f32, f32)>,
    last_tick: Instant,
    last_saved: Instant,
    memory_view: bool,
    error: Option<String>,
    page: Page,
    sidebar_collapsed: bool,
    cpu_history_detail: VecDeque<f32>,
    gpu_history: VecDeque<f32>,
    download_history: VecDeque<f32>,
    upload_history: VecDeque<f32>,
    process_rows: Vec<ProcessRow>,
    process_sort: ProcessSort,
    process_descending: bool,
    process_filter: String,
    selected_process: Option<u32>,
    process_drawer_open: bool,
    drawer_opened_at: Option<Instant>,
    process_history: HashMap<u32, VecDeque<(f32, f32)>>,
    gpu_samples: Vec<GpuSample>,
    ping_ms: Option<f32>,
    jitter_ms: Option<f32>,
    ping_rx: Receiver<(Option<f32>, Option<f32>)>,
    interfaces: Vec<String>,
}

impl App {
    fn new(ctx: &egui::Context) -> Self {
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "roboto-regular".into(),
            egui::FontData::from_static(include_bytes!("../assets/fonts/Roboto-Regular.ttf"))
                .into(),
        );
        fonts.font_data.insert(
            "roboto-bold".into(),
            egui::FontData::from_static(include_bytes!("../assets/fonts/Roboto-Bold.ttf")).into(),
        );
        fonts.families.insert(
            egui::FontFamily::Proportional,
            vec!["roboto-regular".into(), "roboto-bold".into()],
        );
        ctx.set_fonts(fonts);
        let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
        style
            .text_styles
            .iter_mut()
            .for_each(|(_, font)| font.size = (font.size * 1.12).min(22.));
        style.spacing.scroll.fade.strength = 0.;
        ctx.set_style_of(egui::Theme::Dark, style);
        let palette_path = system_scheme_path();
        let palette_modified = palette_path
            .as_deref()
            .and_then(|path| std::fs::metadata(path).ok())
            .and_then(|metadata| metadata.modified().ok());
        if let Some(palette) = palette_path.as_deref().and_then(parse_system_palette) {
            install_palette(palette);
        }
        apply_egui_palette(ctx);
        let mut system = System::new_all().expect("No se pudo inicializar sysinfo");
        system.refresh_all();
        let mut networks = Networks::new_with_refreshed_list().expect("No se pudo leer la red");
        networks.refresh(true);
        let disks =
            Disks::new_with_refreshed_list().unwrap_or_else(|_| Disks::new().expect("Discos"));
        let gpus = Gpus::new_with_refreshed_list().unwrap_or_else(|_| Gpus::new().expect("GPU"));
        let components = Components::new_with_refreshed_list()
            .unwrap_or_else(|_| Components::new().expect("Sensores"));
        let users =
            Users::new_with_refreshed_list().unwrap_or_else(|_| Users::new().expect("Usuarios"));
        let db_path = dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("sys-dashboard");
        let _ = std::fs::create_dir_all(&db_path);
        let database_file = db_path.join("database.sqlite");
        if !database_file.exists() {
            if let Some(config) = dirs::config_dir() {
                for legacy in [
                    config.join("dashboard/database.sqlite"),
                    config.join("Sys Dashboard/database.sqlite"),
                ] {
                    if legacy.exists() {
                        let _ = std::fs::copy(legacy, &database_file);
                        break;
                    }
                }
            }
        }
        let db = Connection::open(&database_file)
            .unwrap_or_else(|_| Connection::open_in_memory().expect("SQLite en memoria"));
        let _ = db.execute_batch("CREATE TABLE IF NOT EXISTS stats (id INTEGER PRIMARY KEY AUTOINCREMENT, cpu REAL, ram REAL, timestamp DATETIME DEFAULT CURRENT_TIMESTAMP)");
        let history = db
            .prepare("SELECT cpu, ram FROM stats ORDER BY timestamp DESC LIMIT 100")
            .and_then(|mut stmt| {
                stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                    .map(|rows| rows.filter_map(Result::ok).collect::<Vec<_>>())
            })
            .unwrap_or_default();
        let history = history.into_iter().rev().collect();
        let (ping_tx, ping_rx) = mpsc::channel();
        std::thread::spawn(move || {
            loop {
                let _ = ping_tx.send(ping_stats());
                std::thread::sleep(Duration::from_secs(5));
            }
        });
        Self {
            palette_path,
            palette_modified,
            last_palette_check: Instant::now(),
            system,
            networks,
            disks,
            gpus,
            components,
            users,
            db,
            sample: Sample {
                cpu: 0.,
                ram_used: 0.,
                ram_total: 0.,
                down: 0,
                up: 0,
            },
            cpu_history: VecDeque::new(),
            memory_history: VecDeque::new(),
            history,
            last_tick: Instant::now() - Duration::from_secs(2),
            last_saved: Instant::now(),
            memory_view: false,
            error: None,
            page: Page::General,
            sidebar_collapsed: false,
            cpu_history_detail: VecDeque::new(),
            gpu_history: VecDeque::new(),
            download_history: VecDeque::new(),
            upload_history: VecDeque::new(),
            process_rows: vec![],
            process_sort: ProcessSort::Cpu,
            process_descending: true,
            process_filter: String::new(),
            selected_process: None,
            process_drawer_open: false,
            drawer_opened_at: None,
            process_history: HashMap::new(),
            gpu_samples: vec![],
            ping_ms: None,
            jitter_ms: None,
            ping_rx,
            interfaces: interface_addresses(),
        }
    }
    fn refresh(&mut self) {
        while let Ok((ping, jitter)) = self.ping_rx.try_recv() {
            self.ping_ms = ping;
            self.jitter_ms = jitter;
        }
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        self.system
            .refresh_processes(sysinfo::ProcessesToUpdate::All, true);
        self.networks.refresh(true);
        self.disks.refresh(true);
        self.gpus.refresh(true);
        self.components.refresh(true);
        self.users.refresh();
        let cpu = self.system.global_cpu_usage();
        let cpu_scale = self.system.cpus().len().max(1) as f32;
        let total = self.system.total_memory() as f64 / 1_073_741_824.;
        let used = self.system.used_memory() as f64 / 1_073_741_824.;
        let totals = self.networks.iter().fold((0u64, 0u64), |a, (_, n)| {
            (a.0 + n.received(), a.1 + n.transmitted())
        });
        let down = totals.0;
        let up = totals.1;
        self.download_history.push_back(down as f32);
        self.upload_history.push_back(up as f32);
        self.process_rows = self
            .system
            .processes()
            .iter()
            .filter(|(_, process)| process.thread_kind().is_none())
            .map(|(pid, p)| {
                let io = p.disk_usage();
                let owner = p
                    .user_id()
                    .and_then(|id| self.users.get_user_by_id(id))
                    .map(|u| u.name().to_owned())
                    .unwrap_or_else(|| "desconocido".into());
                ProcessRow {
                    pid: pid.as_u32(),
                    name: process_program(p),
                    executable: p
                        .exe()
                        .map(|path| path.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    command: p
                        .cmd()
                        .iter()
                        .map(|arg| arg.to_string_lossy())
                        .collect::<Vec<_>>()
                        .join(" "),
                    owner,
                    parent: p.parent().map(|parent| parent.as_u32()),
                    threads: p.tasks().map(|tasks| tasks.len()).unwrap_or(1).max(1),
                    cpu: p.cpu_usage() / cpu_scale,
                    memory: p.memory(),
                    status: format!("{:?}", p.status()),
                    read: io.read_bytes,
                    written: io.written_bytes,
                    runtime: p.run_time(),
                }
            })
            .collect();
        let active_pids: HashSet<_> = self
            .process_rows
            .iter()
            .map(|process| process.pid)
            .collect();
        let total_memory = self.system.total_memory().max(1) as f32;
        for process in &self.process_rows {
            let history = self.process_history.entry(process.pid).or_default();
            history.push_back((process.cpu, process.memory as f32 * 100. / total_memory));
            if history.len() > 120 {
                history.pop_front();
            }
        }
        self.process_history
            .retain(|pid, _| active_pids.contains(pid));
        self.gpu_samples = self
            .gpus
            .iter()
            .map(|gpu| GpuSample {
                name: gpu
                    .model()
                    .or(gpu.vendor())
                    .unwrap_or("GPU desconocida")
                    .to_owned(),
                usage: gpu.usage(),
                used_memory: gpu.used_memory(),
                total_memory: gpu.total_memory(),
            })
            .collect();
        let gpu_usage = self
            .gpu_samples
            .iter()
            .filter_map(|g| g.usage)
            .next()
            .unwrap_or(0.);
        self.sample = Sample {
            cpu,
            ram_used: used,
            ram_total: total,
            down,
            up,
        };
        self.cpu_history.push_back(cpu);
        self.cpu_history_detail.push_back(cpu);
        self.gpu_history.push_back(gpu_usage);
        self.memory_history
            .push_back((used as f32, self.system.used_swap() as f32 / 1_073_741_824.));
        if self.memory_history.len() > 120 {
            self.memory_history.pop_front();
        }
        if self.cpu_history.len() > 60 {
            self.cpu_history.pop_front();
        }
        for values in [
            &mut self.cpu_history_detail,
            &mut self.gpu_history,
            &mut self.download_history,
            &mut self.upload_history,
        ] {
            if values.len() > 120 {
                values.pop_front();
            }
        }
        if self.last_saved.elapsed() >= Duration::from_secs(60) {
            let ram = if total > 0. { used * 100. / total } else { 0. };
            if let Err(e) = self.db.execute(
                "INSERT INTO stats (cpu,ram) VALUES (?1,?2)",
                params![cpu, ram],
            ) {
                self.error = Some(e.to_string());
            } else {
                self.history.push((cpu, ram as f32));
                if self.history.len() > 100 {
                    self.history.remove(0);
                }
            }
            self.last_saved = Instant::now();
        }
    }
}

impl App {
    fn render_page(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(ui, |ui| {
                egui::Frame::new()
                    .fill(Color32::TRANSPARENT)
                    .stroke(Stroke::NONE)
                    .inner_margin(24.)
                    .show(ui, |ui| match self.page {
                        Page::Cpu => self.cpu_page(ui),
                        Page::Memory => self.memory_page(ui),
                        Page::Network => self.network_page(ui),
                        Page::Processes => self.processes_page(ui),
                        Page::General => {}
                    });
            });
    }

    fn page_title(ui: &mut egui::Ui, title: &str, subtitle: &str) {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.heading(
                    egui::RichText::new(title)
                        .color(secondary())
                        .size(24.)
                        .strong(),
                );
                ui.label(egui::RichText::new(subtitle).color(muted()).size(12.));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                clock_widget(ui);
            });
        });
        ui.add_space(16.);
    }

    fn cpu_page(&mut self, ui: &mut egui::Ui) {
        Self::page_title(
            ui,
            "CPU y GPU",
            "Uso, núcleos, sensores y carga del sistema",
        );
        let load = System::load_average().ok();
        let cpu_temp = self
            .components
            .iter()
            .find(|c| c.label().to_lowercase().contains("cpu"))
            .and_then(|c| c.temperature());
        let gpu_temp = self
            .components
            .iter()
            .find(|c| c.label().to_lowercase().contains("gpu"))
            .and_then(|c| c.temperature());
        ui.columns(5, |cols| {
            panel_icon_with_min_height(
                &mut cols[0],
                "CPU total",
                Icon::Cpu,
                secondary(),
                52.,
                |ui| {
                    ui.label(
                        egui::RichText::new(format!("{:.1}%", self.sample.cpu))
                            .size(23.)
                            .strong()
                            .color(secondary()),
                    );
                    ui.add(
                        egui::ProgressBar::new((self.sample.cpu / 100.).clamp(0., 1.))
                            .fill(secondary())
                            .desired_height(10.),
                    );
                },
            );
            panel_icon_with_min_height(
                &mut cols[1],
                "Carga media",
                Icon::Chart,
                secondary(),
                52.,
                |ui| {
                    if let Some(load) = &load {
                        ui.label(format!(
                            "{:.2} · {:.2} · {:.2}",
                            load.one, load.five, load.fifteen
                        ));
                        let cores = self.system.cpus().len().max(1) as f32;
                        ui.add(
                            egui::ProgressBar::new((load.one as f32 / cores).clamp(0., 1.))
                                .fill(secondary())
                                .desired_height(10.),
                        );
                    } else {
                        ui.label("No disponible");
                        ui.add_space(10.);
                    }
                },
            );
            panel_icon_with_min_height(
                &mut cols[2],
                "Núcleos",
                Icon::Cpu,
                secondary(),
                52.,
                |ui| {
                    ui.label(format!(
                        "{} lógicos · {} físicos",
                        self.system.cpus().len(),
                        System::physical_core_count().unwrap_or(0)
                    ));
                },
            );
            panel_icon_with_min_height(&mut cols[3], "GPU", Icon::Gpu, secondary(), 52., |ui| {
                let usage = self.gpu_samples.first().and_then(|g| g.usage);
                ui.label(
                    usage
                        .map(|v| format!("{v:.1}%"))
                        .unwrap_or_else(|| "No disponible".into()),
                );
            });
            panel_icon_with_min_height(
                &mut cols[4],
                "Temperatura",
                Icon::Temperature,
                secondary(),
                52.,
                |ui| {
                    ui.label(format!(
                        "CPU {} · GPU {}",
                        cpu_temp
                            .map(|v| format!("{v:.0}°"))
                            .unwrap_or_else(|| "N/D".into()),
                        gpu_temp
                            .map(|v| format!("{v:.0}°"))
                            .unwrap_or_else(|| "N/D".into())
                    ));
                },
            );
        });
        ui.add_space(16.);
        ui.columns(2, |cols| {
            panel_icon(
                &mut cols[0],
                "Historial de CPU",
                Icon::Chart,
                teal(),
                |ui| chart(ui, &self.cpu_history_detail, teal()),
            );
            panel_icon(
                &mut cols[1],
                "Historial de GPU",
                Icon::Gpu,
                tertiary(),
                |ui| chart(ui, &self.gpu_history, tertiary()),
            );
        });
        ui.add_space(16.);
        panel_icon(ui, "Núcleos de CPU", Icon::Cpu, teal(), |ui| {
            let cpus = self.system.cpus();
            let count = cpus.len().max(1);
            let available = ui.available_width();
            let max_columns = (((available + 8.) / 140.).floor() as usize)
                .clamp(1, 8)
                .min(count);
            let columns = (1..=max_columns)
                .rev()
                .find(|columns| count % columns == 0)
                .unwrap_or(1);
            let tile_width = ((available - 8. * columns.saturating_sub(1) as f32) / columns as f32)
                .clamp(124., 200.);
            let grid_width = tile_width * columns as f32 + 8. * columns.saturating_sub(1) as f32;
            ui.horizontal(|ui| {
                ui.add_space(((available - grid_width) * 0.5).max(0.));
                egui::Grid::new("cpu-core-grid")
                    .num_columns(columns)
                    .spacing(Vec2::new(8., 8.))
                    .show(ui, |grid| {
                        for (index, cpu) in cpus.iter().enumerate() {
                            grid.allocate_ui_with_layout(
                                Vec2::new(tile_width, 88.),
                                egui::Layout::top_down(egui::Align::Min),
                                |tile| {
                                    egui::Frame::new()
                                        .fill(Color32::from_rgba_unmultiplied(
                                            primary().r(),
                                            primary().g(),
                                            primary().b(),
                                            238,
                                        ))
                                        .stroke(Stroke::new(1., glass_border()))
                                        .corner_radius(14.)
                                        .inner_margin(egui::Margin::symmetric(10, 8))
                                        .show(tile, |tile| {
                                            tile.horizontal(|tile| {
                                                tile.label(
                                                    egui::RichText::new(format!("Core {index}"))
                                                        .color(muted())
                                                        .size(13.),
                                                );
                                                tile.with_layout(
                                                    egui::Layout::right_to_left(
                                                        egui::Align::Center,
                                                    ),
                                                    |tile| {
                                                        tile.label(
                                                            egui::RichText::new(format!(
                                                                "{:.0}%",
                                                                cpu.usage()
                                                            ))
                                                            .color(secondary())
                                                            .size(17.)
                                                            .strong(),
                                                        );
                                                    },
                                                );
                                            });
                                            tile.add_space(6.);
                                            tile.add(
                                                egui::ProgressBar::new(cpu.usage() / 100.)
                                                    .fill(secondary())
                                                    .desired_height(9.),
                                            );
                                            tile.add_space(4.);
                                            tile.label(
                                                egui::RichText::new(format!(
                                                    "{} MHz",
                                                    cpu.frequency()
                                                ))
                                                .color(muted())
                                                .size(11.),
                                            );
                                        });
                                },
                            );
                            if (index + 1) % columns == 0 {
                                grid.end_row();
                            }
                        }
                    });
            });
        });
        ui.add_space(12.);
        panel_icon(ui, "GPU detectada", Icon::Gpu, purple(), |ui| {
            if self.gpu_samples.is_empty() {
                ui.label("No se detectó una GPU con telemetría disponible.");
            }
            for gpu in &self.gpu_samples {
                ui.label(&gpu.name);
                ui.label(format!(
                    "Uso: {} · VRAM: {} / {}",
                    gpu.usage
                        .map(|v| format!("{v:.1}%"))
                        .unwrap_or_else(|| "N/D".into()),
                    gpu.used_memory.map(bytes).unwrap_or_else(|| "N/D".into()),
                    gpu.total_memory.map(bytes).unwrap_or_else(|| "N/D".into())
                ));
            }
        });
    }

    fn memory_page(&mut self, ui: &mut egui::Ui) {
        Self::page_title(
            ui,
            "Memoria y discos",
            "RAM, caché, swap y sistemas de archivos montados",
        );
        let total = self.system.total_memory();
        let used = self.system.used_memory();
        let free = self.system.free_memory();
        let cache = meminfo_value("Cached").unwrap_or(0);
        let swap_used = self.system.used_swap();
        let swap_total = self.system.total_swap();
        let row_width = ui.available_width();
        let total_label = bytes(total);
        if row_width < 1120. {
            ui.columns(2, |cols| {
                memory_stat_panel(
                    &mut cols[0],
                    "Memoria usada",
                    bytes(used),
                    used as f32 * 100. / total.max(1) as f32,
                    total_label.clone(),
                    blue(),
                );
                memory_stat_panel(
                    &mut cols[1],
                    "Memoria libre",
                    bytes(free),
                    free as f32 * 100. / total.max(1) as f32,
                    total_label.clone(),
                    teal(),
                );
            });
            ui.add_space(12.);
            ui.columns(2, |cols| {
                memory_cache_panel(&mut cols[0], cache, total);
                memory_swap_panel(&mut cols[1], swap_used, swap_total);
            });
        } else {
            let primary_width = row_width * 0.58;
            let detail_width = row_width - primary_width - 12.;
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(
                    Vec2::new(primary_width, 146.),
                    egui::Layout::left_to_right(egui::Align::Min),
                    |group| {
                        let card_width = (group.available_width() - 12.) / 2.;
                        group.allocate_ui_with_layout(
                            Vec2::new(card_width, 146.),
                            egui::Layout::top_down(egui::Align::Min),
                            |card| {
                                memory_stat_panel(
                                    card,
                                    "Memoria usada",
                                    bytes(used),
                                    used as f32 * 100. / total.max(1) as f32,
                                    total_label.clone(),
                                    blue(),
                                );
                            },
                        );
                        group.add_space(12.);
                        group.allocate_ui_with_layout(
                            Vec2::new(card_width, 146.),
                            egui::Layout::top_down(egui::Align::Min),
                            |card| {
                                memory_stat_panel(
                                    card,
                                    "Memoria libre",
                                    bytes(free),
                                    free as f32 * 100. / total.max(1) as f32,
                                    total_label.clone(),
                                    teal(),
                                );
                            },
                        );
                    },
                );
                ui.add_space(12.);
                ui.allocate_ui_with_layout(
                    Vec2::new(detail_width, 146.),
                    egui::Layout::left_to_right(egui::Align::Min),
                    |group| {
                        let card_width = (group.available_width() - 12.) / 2.;
                        group.allocate_ui_with_layout(
                            Vec2::new(card_width, 146.),
                            egui::Layout::top_down(egui::Align::Min),
                            |card| memory_cache_panel(card, cache, total),
                        );
                        group.add_space(12.);
                        group.allocate_ui_with_layout(
                            Vec2::new(card_width, 146.),
                            egui::Layout::top_down(egui::Align::Min),
                            |card| memory_swap_panel(card, swap_used, swap_total),
                        );
                    },
                );
            });
        }
        ui.add_space(16.);
        panel_icon(ui, "Discos montados", Icon::Disk, blue(), |ui| {
            let available_width = ui.available_width();
            let columns = ((available_width + 12.) / 292.).floor().max(1.) as usize;
            let columns = columns.min(self.disks.len().max(1)).min(4);
            let card_width = ((available_width - 12. * (columns.saturating_sub(1) as f32))
                / columns as f32)
                .min(420.);
            ui.horizontal_wrapped(|ui| {
                for disk in &self.disks {
                    let total = disk.total_space();
                    let available = disk.available_space();
                    let used = total.saturating_sub(available);
                    let pct = used as f32 / total.max(1) as f32;
                    ui.allocate_ui_with_layout(
                        Vec2::new(card_width, 142.),
                        egui::Layout::top_down(egui::Align::Min),
                        |card| {
                            egui::Frame::new()
                                .fill(with_alpha(secondary(), 22))
                                .stroke(Stroke::new(1., glass_border()))
                                .corner_radius(14.)
                                .inner_margin(14.)
                                .show(card, |card| {
                                    card.label(
                                        egui::RichText::new(
                                            disk.mount_point().display().to_string(),
                                        )
                                        .size(15.)
                                        .strong()
                                        .color(secondary()),
                                    );
                                    card.label(
                                        egui::RichText::new(disk.name().to_string_lossy())
                                            .color(muted())
                                            .size(12.),
                                    );
                                    card.add_space(6.);
                                    card.label(
                                        egui::RichText::new(format!("{:.0}% usado", pct * 100.))
                                            .size(12.)
                                            .strong()
                                            .color(secondary()),
                                    );
                                    card.add_space(3.);
                                    card.add(
                                        egui::ProgressBar::new(pct)
                                            .fill(if pct > 0.9 {
                                                Color32::from_rgb(205, 116, 104)
                                            } else {
                                                blue()
                                            })
                                            .desired_height(18.),
                                    );
                                    card.add_space(5.);
                                    card.horizontal(|card| {
                                        card.label(
                                            egui::RichText::new(format!("{} usados", bytes(used)))
                                                .size(12.)
                                                .color(secondary()),
                                        );
                                        card.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |card| {
                                                card.label(
                                                    egui::RichText::new(format!(
                                                        "{} libres",
                                                        bytes(available)
                                                    ))
                                                    .size(12.)
                                                    .color(secondary()),
                                                );
                                            },
                                        );
                                    });
                                });
                        },
                    );
                }
            });
        });
        ui.add_space(14.);
        panel_icon(ui, "Historial de RAM", Icon::Memory, blue(), |ui| {
            memory_chart(ui, &self.memory_history, false);
        });
        ui.add_space(12.);
        panel_icon(ui, "Historial de Swap", Icon::Memory, secondary(), |ui| {
            memory_chart(ui, &self.memory_history, true)
        });
    }

    fn network_page(&mut self, ui: &mut egui::Ui) {
        Self::page_title(
            ui,
            "Red",
            "Tráfico, calidad de conexión e interfaces activas",
        );
        let max_down = self.download_history.iter().copied().fold(0., f32::max);
        let max_up = self.upload_history.iter().copied().fold(0., f32::max);
        let avg_down = average(&self.download_history);
        let avg_up = average(&self.upload_history);
        let (total_down, total_up) = self.networks.iter().fold((0u64, 0u64), |a, (_, n)| {
            (a.0 + n.total_received(), a.1 + n.total_transmitted())
        });
        panel_icon(ui, "Interfaces e IP", Icon::Network, purple(), |ui| {
            ui.horizontal_wrapped(|ui| {
                if self.interfaces.is_empty() {
                    ui.label("Sin IP detectada");
                }
                for address in &self.interfaces {
                    egui::Frame::new()
                        .fill(with_alpha(secondary(), 18))
                        .corner_radius(8.)
                        .inner_margin(egui::Margin::symmetric(10, 6))
                        .show(ui, |ui| {
                            ui.monospace(address);
                        });
                }
            });
            ui.label(format!(
                "Acumulado · ↓ {}  ↑ {}",
                bytes(total_down),
                bytes(total_up)
            ));
        });
        ui.add_space(12.);
        ui.columns(4, |cols| {
            panel_icon(&mut cols[0], "Descarga", Icon::Download, teal(), |ui| {
                ui.label(speed(self.sample.down));
                ui.label(format!(
                    "Máx. {} · Prom. {}",
                    speed(max_down as u64),
                    speed(avg_down as u64)
                ));
            });
            panel_icon(&mut cols[1], "Subida", Icon::Upload, blue(), |ui| {
                ui.label(speed(self.sample.up));
                ui.label(format!(
                    "Máx. {} · Prom. {}",
                    speed(max_up as u64),
                    speed(avg_up as u64)
                ));
            });
            panel_icon(&mut cols[2], "Ping", Icon::Network, purple(), |ui| {
                ui.label(
                    self.ping_ms
                        .map(|v| format!("{v:.1} ms"))
                        .unwrap_or_else(|| "No disponible".into()),
                );
            });
            panel_icon(&mut cols[3], "Jitter", Icon::Network, secondary(), |ui| {
                ui.label(
                    self.jitter_ms
                        .map(|v| format!("{v:.1} ms"))
                        .unwrap_or_else(|| "No disponible".into()),
                );
            });
        });
        ui.add_space(16.);
        ui.columns(2, |cols| {
            panel_icon(&mut cols[0], "Descarga", Icon::Download, teal(), |ui| {
                chart_scaled(ui, &self.download_history, teal())
            });
            panel_icon(&mut cols[1], "Subida", Icon::Upload, blue(), |ui| {
                chart_scaled(ui, &self.upload_history, blue())
            });
        });
    }

    fn processes_page(&mut self, ui: &mut egui::Ui) {
        Self::page_title(
            ui,
            "Procesos",
            "Filtrá, ordená y seleccioná un proceso para abrir sus detalles",
        );
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), 42.),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.add_sized(
                    [300., 40.],
                    egui::TextEdit::singleline(&mut self.process_filter)
                        .hint_text("Filtrar por nombre, comando o dueño")
                        .align(egui::Align2::LEFT_CENTER)
                        .frame(
                            egui::Frame::new()
                                .fill(glass_strong())
                                .stroke(Stroke::new(1., glass_border()))
                                .corner_radius(14.)
                                .inner_margin(egui::Margin::symmetric(12, 7)),
                        ),
                );
                ui.add_space(4.);
                for (sort, label) in [
                    (ProcessSort::Cpu, "CPU"),
                    (ProcessSort::Memory, "RAM"),
                    (ProcessSort::Pid, "PID"),
                    (ProcessSort::Name, "Programa"),
                    (ProcessSort::Arguments, "Comando"),
                    (ProcessSort::Threads, "Hilos"),
                    (ProcessSort::Owner, "Usuario"),
                ] {
                    let selected = self.process_sort == sort;
                    if ui
                        .add(
                            egui::Button::new(egui::RichText::new(label).size(13.))
                                .selected(selected)
                                .fill(if selected {
                                    with_alpha(tertiary(), 78)
                                } else {
                                    with_alpha(secondary(), 20)
                                })
                                .stroke(Stroke::new(
                                    1.,
                                    if selected {
                                        with_alpha(tertiary(), 110)
                                    } else {
                                        with_alpha(secondary(), 38)
                                    },
                                ))
                                .corner_radius(14.)
                                .min_size(Vec2::new(50., 38.)),
                        )
                        .clicked()
                    {
                        if self.process_sort == sort {
                            self.process_descending = !self.process_descending;
                        } else {
                            self.process_sort = sort;
                            self.process_descending = matches!(
                                sort,
                                ProcessSort::Cpu | ProcessSort::Memory | ProcessSort::Threads
                            );
                        }
                    }
                }
            },
        );
        let filter = self.process_filter.to_lowercase();
        let mut rows: Vec<_> = self
            .process_rows
            .iter()
            .filter(|p| {
                filter.is_empty()
                    || p.name.to_lowercase().contains(&filter)
                    || p.command.to_lowercase().contains(&filter)
                    || p.executable.to_lowercase().contains(&filter)
                    || p.owner.to_lowercase().contains(&filter)
            })
            .cloned()
            .collect();
        rows.sort_by(|a, b| {
            let sort = match self.process_sort {
                ProcessSort::Cpu => a
                    .cpu
                    .total_cmp(&b.cpu)
                    .then_with(|| a.memory.cmp(&b.memory))
                    .then_with(|| b.pid.cmp(&a.pid)),
                ProcessSort::Memory => a
                    .memory
                    .cmp(&b.memory)
                    .then_with(|| a.cpu.total_cmp(&b.cpu))
                    .then_with(|| b.pid.cmp(&a.pid)),
                ProcessSort::Pid => a.pid.cmp(&b.pid),
                ProcessSort::Name => a.name.cmp(&b.name),
                ProcessSort::Arguments => a.command.cmp(&b.command),
                ProcessSort::Threads => a.threads.cmp(&b.threads),
                ProcessSort::Owner => a.owner.cmp(&b.owner),
            };
            if self.process_descending {
                sort.reverse()
            } else {
                sort
            }
        });
        ui.add_space(12.);
        panel_icon(ui, "Procesos activos", Icon::Processes, blue(), |ui| {
            let table_width = ui.available_width();
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("{} procesos", rows.len()))
                        .color(muted())
                        .size(12.),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(if self.process_descending {
                            "↓"
                        } else {
                            "↑"
                        })
                        .color(teal())
                        .size(13.),
                    );
                    ui.label(
                        egui::RichText::new(sort_label(self.process_sort))
                            .color(muted())
                            .size(12.),
                    );
                });
            });
            let fractions = [0.08, 0.15, 0.36, 0.08, 0.11, 0.12, 0.10];
            let widths: Vec<_> = fractions
                .iter()
                .map(|fraction| table_width * fraction)
                .collect();
            let headers = [
                "PID", "PROGRAMA", "COMANDO", "HILOS", "USUARIO", "MEMORIA", "CPU",
            ];
            let right = [0, 3, 5, 6];
            process_table_header(ui, &widths, &headers, &right);
            ui.add_space(4.);
            for row in &rows {
                let values = [
                    row.pid.to_string(),
                    row.name.clone(),
                    if row.command.is_empty() {
                        "—".into()
                    } else {
                        row.command.clone()
                    },
                    row.threads.to_string(),
                    row.owner.clone(),
                    compact_bytes(row.memory),
                    format!("{:.1}%", row.cpu),
                ];
                if process_table_row(
                    ui,
                    &widths,
                    &values,
                    &right,
                    self.selected_process == Some(row.pid),
                    46.,
                ) {
                    self.selected_process = Some(row.pid);
                    self.process_drawer_open = true;
                    self.drawer_opened_at = Some(Instant::now());
                }
            }
        });
    }
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0., 0., 0., 0.]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.last_palette_check.elapsed() >= Duration::from_millis(500) {
            self.last_palette_check = Instant::now();
            if let Some(path) = self.palette_path.as_deref() {
                let modified = std::fs::metadata(path)
                    .ok()
                    .and_then(|metadata| metadata.modified().ok());
                if modified.is_some() && modified != self.palette_modified {
                    if let Some(palette) = parse_system_palette(path) {
                        install_palette(palette);
                        apply_egui_palette(&ctx);
                        self.palette_modified = modified;
                        ctx.request_repaint();
                    }
                }
            }
        }
        let drawer_age = self
            .drawer_opened_at
            .map(|opened| opened.elapsed())
            .unwrap_or_default();
        let refresh_interval = Duration::from_secs(1);
        if self.last_tick.elapsed() >= refresh_interval
            && (!self.process_drawer_open || drawer_age >= Duration::from_millis(1_400))
        {
            self.refresh();
            self.last_tick = Instant::now();
        }
        ctx.request_repaint_after(Duration::from_millis(200));
        let viewport = ui.max_rect();
        let sidebar_width = ctx.animate_value_with_time(
            egui::Id::NULL.with(egui::IdSalt::new("sidebar-width")),
            if self.sidebar_collapsed { 76. } else { 210. },
            0.22,
        );
        let sidebar_compact = sidebar_width < 150.;
        egui::Panel::left("sidebar")
            .size_range(sidebar_width..=sidebar_width)
            .resizable(false)
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(Color32::TRANSPARENT)
                    .stroke(Stroke::NONE)
                    .inner_margin(if sidebar_compact { 8. } else { 16. }),
            )
            .show(ui, |ui| {
                paint_glass_surface(
                    ui,
                    Rect::from_min_size(viewport.min, Vec2::new(sidebar_width, viewport.height())),
                    22.,
                );
                ui.add_space(12.);
                if sidebar_compact {
                    ui.vertical_centered(|ui| {
                        icon(ui, Icon::Dashboard, tertiary(), 24.);
                        ui.add_space(10.);
                        sidebar_toggle(ui, &mut self.sidebar_collapsed);
                    });
                } else {
                    ui.horizontal(|ui| {
                        icon(ui, Icon::Dashboard, tertiary(), 26.);
                        ui.label(
                            egui::RichText::new("SysDash")
                                .color(secondary())
                                .size(21.)
                                .strong(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            sidebar_toggle(ui, &mut self.sidebar_collapsed);
                        });
                    });
                }
                ui.add_space(if sidebar_compact { 14. } else { 26. });
                ui.separator();
                ui.add_space(14.);
                if !sidebar_compact {
                    ui.label(
                        egui::RichText::new("MONITOR")
                            .color(muted())
                            .size(10.)
                            .strong(),
                    );
                    ui.add_space(6.);
                }
                for (kind, label) in [
                    (Icon::Dashboard, "General"),
                    (Icon::Cpu, "CPU"),
                    (Icon::Memory, "Memoria"),
                    (Icon::Network, "Red"),
                ] {
                    let page = match label {
                        "CPU" => Page::Cpu,
                        "Memoria" => Page::Memory,
                        "Red" => Page::Network,
                        _ => Page::General,
                    };
                    let selected = self.page == page;
                    let (rect, response) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), 38.),
                        egui::Sense::click(),
                    );
                    let hovered = response.hovered();
                    if selected || hovered {
                        ui.painter().rect_filled(
                            rect,
                            10.,
                            if selected {
                                with_alpha(tertiary(), 52)
                            } else {
                                with_alpha(secondary(), 18)
                            },
                        );
                    }
                    let icon_color = if selected { tertiary() } else { muted() };
                    let icon_x = if sidebar_compact {
                        rect.center().x - 8.5
                    } else {
                        rect.min.x + 11.
                    };
                    let icon_rect = Rect::from_min_size(
                        Pos2::new(icon_x, rect.center().y - 8.5),
                        Vec2::splat(17.),
                    );
                    paint_icon(ui, kind, icon_color, icon_rect);
                    if !sidebar_compact {
                        ui.painter().text(
                            rect.min + Vec2::new(40., rect.height() * 0.5),
                            egui::Align2::LEFT_CENTER,
                            label,
                            egui::FontId::proportional(15.),
                            if selected { tertiary() } else { muted() },
                        );
                    }
                    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
                    if sidebar_compact {
                        response.clone().on_hover_text(label);
                    }
                    if response.clicked() {
                        self.page = page;
                    }
                }
                if !sidebar_compact {
                    ui.add_space(16.);
                    ui.label(
                        egui::RichText::new("SISTEMA")
                            .color(muted())
                            .size(10.)
                            .strong(),
                    );
                    ui.add_space(8.);
                }
                let (rect, processes) = ui.allocate_exact_size(
                    Vec2::new(ui.available_width(), 38.),
                    egui::Sense::click(),
                );
                if self.page == Page::Processes || processes.hovered() {
                    ui.painter().rect_filled(
                        rect,
                        10.,
                        if self.page == Page::Processes {
                            with_alpha(tertiary(), 48)
                        } else {
                            with_alpha(secondary(), 18)
                        },
                    );
                }
                paint_icon(
                    ui,
                    Icon::Processes,
                    if self.page == Page::Processes {
                        teal()
                    } else {
                        muted()
                    },
                    Rect::from_min_size(
                        Pos2::new(
                            if sidebar_compact {
                                rect.center().x - 8.5
                            } else {
                                rect.min.x + 11.
                            },
                            rect.center().y - 8.5,
                        ),
                        Vec2::splat(17.),
                    ),
                );
                if !sidebar_compact {
                    ui.painter().text(
                        rect.min + Vec2::new(40., rect.height() * 0.5),
                        egui::Align2::LEFT_CENTER,
                        "Procesos",
                        egui::FontId::proportional(15.),
                        muted(),
                    );
                }
                let processes = processes.on_hover_cursor(egui::CursorIcon::PointingHand);
                if sidebar_compact {
                    processes.clone().on_hover_text("Procesos");
                }
                if processes.clicked() {
                    self.page = Page::Processes;
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.horizontal(|ui| {
                        let p = ui.painter();
                        let c = ui.cursor().center();
                        p.circle_filled(c, 4., teal());
                        if !sidebar_compact {
                            ui.add_space(10.);
                            ui.label(egui::RichText::new("Conectado").color(muted()).size(11.));
                        }
                    });
                });
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(Color32::TRANSPARENT)
                    .stroke(Stroke::NONE)
                    .inner_margin(0.),
            )
            .show(ui, |ui| {
                if self.page != Page::General {
                    self.render_page(ui);
                    return;
                }
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .show(ui, |ui| {
                        egui::Frame::new()
                            .fill(Color32::TRANSPARENT)
                            .stroke(Stroke::NONE)
                            .inner_margin(24.)
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.vertical(|ui| {
                                        ui.heading(
                                            egui::RichText::new("Panel General")
                                                .color(secondary())
                                                .size(22.)
                                                .strong(),
                                        );
                                        ui.label(
                                            egui::RichText::new("Monitoreo en tiempo real")
                                                .color(muted())
                                                .size(11.),
                                        );
                                    });
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            clock_widget(ui);
                                        },
                                    );
                                });
                                ui.add_space(16.);
                                ui.columns(4, |cols| {
                                    let ram = if self.sample.ram_total > 0. {
                                        self.sample.ram_used * 100. / self.sample.ram_total
                                    } else {
                                        0.
                                    };
                                    metric(
                                        &mut cols[0],
                                        "CPU",
                                        format!("{:.1}%", self.sample.cpu),
                                        self.sample.cpu,
                                        teal(),
                                    );
                                    metric(
                                        &mut cols[1],
                                        "RAM",
                                        format!("{:.1} GB", self.sample.ram_used),
                                        ram as f32,
                                        blue(),
                                    );
                                    panel_icon(
                                        &mut cols[2],
                                        "RED",
                                        Icon::Network,
                                        purple(),
                                        |ui| {
                                            ui.horizontal(|ui| {
                                                icon(ui, Icon::Download, teal(), 13.);
                                                ui.colored_label(teal(), speed(self.sample.down));
                                            });
                                            ui.horizontal(|ui| {
                                                icon(ui, Icon::Upload, blue(), 13.);
                                                ui.colored_label(blue(), speed(self.sample.up));
                                            });
                                        },
                                    );
                                    let (battery, _charging) = battery_info();
                                    panel_icon(
                                        &mut cols[3],
                                        "BATERÍA",
                                        Icon::Battery,
                                        secondary(),
                                        |ui| {
                                            if let Some(level) = battery {
                                                ui.label(
                                                    egui::RichText::new(format!("{level}%"))
                                                        .size(23.)
                                                        .strong()
                                                        .color(secondary()),
                                                );
                                                ui.add(
                                                    egui::ProgressBar::new(level as f32 / 100.)
                                                        .fill(secondary()),
                                                );
                                            } else {
                                                ui.label(
                                                    egui::RichText::new("N/A")
                                                        .size(23.)
                                                        .strong()
                                                        .color(muted()),
                                                );
                                            }
                                        },
                                    );
                                });
                                ui.add_space(16.);
                                let width = ui.available_width();
                                let chart_row_height = ui.available_height().max(320.);
                                ui.horizontal(|ui| {
                                    ui.allocate_ui_with_layout(
                                        Vec2::new(width * 0.62, chart_row_height),
                                        egui::Layout::top_down(egui::Align::Min),
                                        |ui| {
                                            panel_icon(ui, "Uso de CPU", Icon::Cpu, teal(), |ui| {
                                                ui.horizontal(|ui| {
                                                    ui.label(
                                                        egui::RichText::new("Últimos 20 puntos")
                                                            .color(muted())
                                                            .size(10.),
                                                    );
                                                    ui.with_layout(
                                                        egui::Layout::right_to_left(
                                                            egui::Align::Center,
                                                        ),
                                                        |ui| {
                                                            ui.colored_label(teal(), "LIVE");
                                                        },
                                                    );
                                                });
                                                chart(ui, &self.cpu_history, teal());
                                            });
                                        },
                                    );
                                    ui.allocate_ui_with_layout(
                                        Vec2::new(width * 0.38 - 8., chart_row_height),
                                        egui::Layout::top_down(egui::Align::Min),
                                        |ui| {
                                            panel_icon(
                                                ui,
                                                "Historial",
                                                Icon::Chart,
                                                purple(),
                                                |ui| {
                                                    ui.label(
                                                        egui::RichText::new("CPU & RAM promedio")
                                                            .color(muted())
                                                            .size(10.),
                                                    );
                                                    history_chart(ui, &self.history);
                                                },
                                            );
                                        },
                                    );
                                });
                                ui.add_space(16.);
                                panel_icon(ui, "Procesos", Icon::Processes, blue(), |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new("Cinco con mayor consumo")
                                                .color(muted())
                                                .size(12.),
                                        );
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                for (memory, label) in
                                                    [(true, "RAM"), (false, "CPU")]
                                                {
                                                    let active = self.memory_view == memory;
                                                    if ui
                                                        .add(
                                                            egui::Button::new(
                                                                egui::RichText::new(label)
                                                                    .size(12.),
                                                            )
                                                            .selected(active)
                                                            .fill(if active {
                                                                Color32::from_rgba_unmultiplied(
                                                                    tertiary().r(),
                                                                    tertiary().g(),
                                                                    tertiary().b(),
                                                                    118,
                                                                )
                                                            } else {
                                                                Color32::from_rgba_unmultiplied(
                                                                    secondary().r(),
                                                                    secondary().g(),
                                                                    secondary().b(),
                                                                    20,
                                                                )
                                                            })
                                                            .corner_radius(12.),
                                                        )
                                                        .clicked()
                                                    {
                                                        self.memory_view = memory;
                                                    }
                                                }
                                            },
                                        );
                                    });
                                    ui.add_space(8.);
                                    let mut list: Vec<_> = self.process_rows.iter().collect();
                                    if self.memory_view {
                                        list.sort_by(|a, b| {
                                            b.memory
                                                .cmp(&a.memory)
                                                .then_with(|| b.cpu.total_cmp(&a.cpu))
                                        });
                                    } else {
                                        list.sort_by(|a, b| {
                                            b.cpu
                                                .total_cmp(&a.cpu)
                                                .then_with(|| b.memory.cmp(&a.memory))
                                        });
                                    }
                                    list.truncate(5);
                                    let table_width = ui.available_width();
                                    let fractions = [0.11, 0.28, 0.34, 0.14, 0.13];
                                    let widths: Vec<_> = fractions
                                        .iter()
                                        .map(|fraction| table_width * fraction)
                                        .collect();
                                    let headers = ["PID", "PROGRAMA", "COMANDO", "CPU", "MEMORIA"];
                                    let right = [0, 3, 4];
                                    process_table_header(ui, &widths, &headers, &right);
                                    for row in list {
                                        let values = [
                                            row.pid.to_string(),
                                            row.name.clone(),
                                            if row.command.is_empty() {
                                                "—".into()
                                            } else {
                                                row.command.clone()
                                            },
                                            format!("{:.1}%", row.cpu),
                                            compact_bytes(row.memory),
                                        ];
                                        if process_table_row(
                                            ui,
                                            &widths,
                                            &values,
                                            &right,
                                            self.selected_process == Some(row.pid),
                                            42.,
                                        ) {
                                            self.selected_process = Some(row.pid);
                                            self.process_drawer_open = true;
                                            self.drawer_opened_at = Some(Instant::now());
                                        }
                                    }
                                    ui.add_space(5.);
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if icon_action_button(
                                                ui,
                                                "Ver todos los procesos",
                                                Icon::Processes,
                                                primary(),
                                            )
                                            .clicked()
                                            {
                                                self.page = Page::Processes;
                                            }
                                        },
                                    );
                                });
                                if let Some(err) = &self.error {
                                    ui.colored_label(Color32::LIGHT_RED, format!("SQLite: {err}"));
                                }
                            });
                    });
            });
        self.draw_process_drawer(ui.ctx(), viewport);
    }
}

impl App {
    fn draw_process_drawer(&mut self, ctx: &egui::Context, viewport: Rect) {
        let backdrop_id = egui::Id::NULL.with(egui::IdSalt::new("process-drawer-backdrop"));
        let drawer_area_id = egui::Id::NULL.with(egui::IdSalt::new("process-drawer-area"));
        let drawer_animation_id = egui::Id::NULL.with(egui::IdSalt::new("process-drawer-slide"));
        let drawer_age = self
            .drawer_opened_at
            .map(|opened| opened.elapsed().as_secs_f32())
            .unwrap_or(1.);
        let progress = ctx.animate_bool_with_time_and_easing(
            drawer_animation_id,
            self.process_drawer_open,
            0.5,
            egui::emath::easing::cubic_in_out,
        );
        if (self.process_drawer_open && drawer_age < 1.45) || (progress > 0.001 && progress < 0.999)
        {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        let stage = |name: &'static str, delay: f32| {
            let animated = ctx.animate_bool_with_time(
                drawer_animation_id.with(egui::IdSalt::new(name)),
                self.process_drawer_open && drawer_age >= delay,
                0.18,
            );
            if !self.process_drawer_open && progress > 0.001 {
                1.
            } else {
                animated
            }
        };
        let title_alpha = stage("process-drawer-title", 0.54);
        let metrics_alpha = [
            stage("process-drawer-metrics-1", 0.56),
            stage("process-drawer-metrics-2", 0.66),
            stage("process-drawer-metrics-3", 0.76),
        ];
        let details_alpha = stage("process-drawer-details", 0.85);
        let io_alpha = stage("process-drawer-io", 0.95);
        let chart_alpha = stage("process-drawer-chart", 1.04);
        let action_alpha = stage("process-drawer-action", 1.14);
        if !self.process_drawer_open && progress <= 0.001 {
            self.selected_process = None;
            return;
        }
        let Some(row) = self
            .selected_process
            .and_then(|pid| self.process_rows.iter().find(|row| row.pid == pid))
            .cloned()
        else {
            self.process_drawer_open = false;
            self.selected_process = None;
            return;
        };

        egui::Area::new(backdrop_id)
            .order(egui::Order::Foreground)
            .fixed_pos(viewport.min)
            .movable(false)
            .show(ctx, |ui| {
                ui.set_min_size(viewport.size());
                let (rect, response) =
                    ui.allocate_exact_size(viewport.size(), egui::Sense::click());
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 0., with_alpha(primary(), (progress * 198.) as u8));
                if response.clicked() {
                    self.process_drawer_open = false;
                }
            });

        let drawer_width = (viewport.width() * 0.36)
            .clamp(410., 520.)
            .min(viewport.width() - 36.);
        let drawer_height = (viewport.height() - 36.).max(300.);
        // Keep the panel fully offscreen at the start; the drawer itself stays
        // opaque while it travels into its resting position.
        let drawer_position = Pos2::new(
            viewport.right() + 18. - (drawer_width + 36.) * progress,
            viewport.top() + 18.,
        );
        egui::Area::new(drawer_area_id)
            .order(egui::Order::Foreground)
            .fixed_pos(drawer_position)
            .constrain(false)
            .movable(false)
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(
                    Vec2::new(drawer_width, drawer_height),
                    egui::Sense::hover(),
                );
                paint_glass_surface(ui, rect, 26.);
                let inner = rect.shrink(24.);
                let action_size = Vec2::new(inner.width(), 42.);
                let action_rect = Rect::from_min_size(
                    Pos2::new(inner.left(), inner.bottom() - action_size.y),
                    action_size,
                );
                let content_rect = Rect::from_min_max(
                    inner.min,
                    Pos2::new(inner.right(), action_rect.top() - 12.),
                );
                let mut panel = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(content_rect)
                        .layout(egui::Layout::top_down(egui::Align::Min))
                        .id_salt(("process-drawer-content", row.pid)),
                );
                panel.set_clip_rect(content_rect);
                panel.scope(|ui| {
                    ui.set_opacity(title_alpha);
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new(&row.name)
                                    .size(22.)
                                    .strong()
                                    .color(secondary()),
                            );
                            ui.label(
                                egui::RichText::new(format!(
                                    "Proceso {} · {}",
                                    row.pid, row.status
                                ))
                                .size(13.)
                                .color(muted()),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                            if ui
                                .add(
                                    egui::Button::new(egui::RichText::new("×").size(22.))
                                        .corner_radius(18.)
                                        .min_size(Vec2::splat(38.)),
                                )
                                .clicked()
                            {
                                self.process_drawer_open = false;
                            }
                        });
                    });
                });
                panel.add_space(14.);
                let col_width = (inner.width() - 20.) / 2.;
                let metrics = [
                    ("CPU", format!("{:.1}%", row.cpu), teal()),
                    ("Memoria", compact_bytes(row.memory), blue()),
                    ("Hilos", row.threads.to_string(), purple()),
                    ("Usuario", row.owner.clone(), muted()),
                    (
                        "Padre",
                        row.parent
                            .map(|pid| pid.to_string())
                            .unwrap_or_else(|| "—".into()),
                        muted(),
                    ),
                    ("Activo", format_duration(row.runtime), muted()),
                ];
                for row_index in 0..3 {
                    panel.scope(|ui| {
                        ui.set_opacity(metrics_alpha[row_index]);
                        ui.horizontal(|ui| {
                            let (left, right) =
                                (&metrics[row_index * 2], &metrics[row_index * 2 + 1]);
                            process_detail_chip(ui, col_width, left.0, left.1.clone(), left.2);
                            ui.add_space(12.);
                            process_detail_chip(ui, col_width, right.0, right.1.clone(), right.2);
                        });
                    });
                    panel.add_space(8.);
                }
                panel.scope(|ui| {
                    ui.set_opacity(details_alpha);
                    ui.separator();
                    detail_section(
                        ui,
                        "Ejecutable",
                        if row.executable.is_empty() {
                            "No disponible"
                        } else {
                            &row.executable
                        },
                    );
                    detail_section(
                        ui,
                        "Comando",
                        if row.command.is_empty() {
                            "No disponible"
                        } else {
                            &row.command
                        },
                    );
                });
                panel.scope(|ui| {
                    ui.set_opacity(io_alpha);
                    ui.add_space(12.);
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new("Lectura").color(muted()).size(12.));
                            ui.label(
                                egui::RichText::new(bytes(row.read))
                                    .color(secondary())
                                    .strong(),
                            );
                        });
                        ui.add_space(28.);
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new("Escritura").color(muted()).size(12.));
                            ui.label(
                                egui::RichText::new(bytes(row.written))
                                    .color(secondary())
                                    .strong(),
                            );
                        });
                    });
                });
                panel.scope(|ui| {
                    ui.set_opacity(chart_alpha);
                    ui.add_space(12.);
                    ui.label(
                        egui::RichText::new("Historial de CPU y memoria")
                            .color(secondary())
                            .size(14.)
                            .strong(),
                    );
                    let history = self.process_history.get(&row.pid);
                    if let Some(history) = history {
                        let graph_height = ui.available_height().clamp(80., 280.);
                        process_history_chart(ui, history, graph_height);
                    } else {
                        ui.label(egui::RichText::new("Recopilando datos…").color(muted()));
                    }
                });
                let mut footer = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(action_rect)
                        .layout(egui::Layout::top_down(egui::Align::Min))
                        .id_salt(("process-drawer-action", row.pid)),
                );
                footer.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.scope(|ui| {
                        ui.set_opacity(action_alpha);
                        if ui
                            .add_sized(
                                [action_size.x, action_size.y],
                                egui::Button::new(
                                    egui::RichText::new("Terminar proceso")
                                        .color(Color32::from_rgb(255, 158, 158))
                                        .strong(),
                                )
                                .fill(Color32::from_rgba_unmultiplied(173, 54, 66, 58))
                                .stroke(Stroke::new(
                                    1.,
                                    Color32::from_rgba_unmultiplied(255, 130, 140, 85),
                                ))
                                .corner_radius(14.),
                            )
                            .clicked()
                        {
                            if let Some(process) =
                                self.system.process(sysinfo::Pid::from(row.pid as usize))
                            {
                                if !process.kill() {
                                    self.error = Some(format!(
                                        "No se pudo finalizar el proceso {}",
                                        row.pid
                                    ));
                                } else {
                                    self.process_drawer_open = false;
                                }
                            }
                        }
                    });
                });
            });
        ctx.move_to_top(egui::LayerId::new(egui::Order::Foreground, drawer_area_id));
    }
}

#[derive(Clone, Copy)]
enum Icon {
    Dashboard,
    SidebarCollapse,
    SidebarExpand,
    Cpu,
    Memory,
    Network,
    Processes,
    Battery,
    Download,
    Upload,
    Chart,
    Gpu,
    Temperature,
    Disk,
}

fn sidebar_toggle(ui: &mut egui::Ui, collapsed: &mut bool) {
    let icon = if *collapsed {
        Icon::SidebarExpand
    } else {
        Icon::SidebarCollapse
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(30.), egui::Sense::click());
    ui.painter()
        .rect_filled(rect, 9., with_alpha(secondary(), 14));
    ui.painter().rect_stroke(
        rect,
        9.,
        Stroke::new(1., glass_border()),
        egui::StrokeKind::Inside,
    );
    paint_icon(ui, icon, tertiary(), rect.shrink(7.));
    let response = response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(if *collapsed {
            "Expandir barra lateral"
        } else {
            "Contraer barra lateral"
        });
    if response.clicked() {
        *collapsed = !*collapsed;
    }
}

fn panel_icon(
    ui: &mut egui::Ui,
    title: &str,
    kind: Icon,
    color: Color32,
    add: impl FnMut(&mut egui::Ui),
) {
    panel_icon_with_min_height(ui, title, kind, color, 0., add);
}

fn panel_icon_with_min_height(
    ui: &mut egui::Ui,
    title: &str,
    kind: Icon,
    color: Color32,
    min_content_height: f32,
    mut add: impl FnMut(&mut egui::Ui),
) {
    let panel_id = ui.next_auto_id();
    let width = ui.available_width();
    let content_rect = Rect::from_min_size(
        ui.cursor().min + Vec2::splat(16.),
        Vec2::new((width - 32.).max(1.), 16_384.),
    );
    let mut measure = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(content_rect)
            .layout(egui::Layout::top_down(egui::Align::Min))
            .id_salt(("glass-measure", panel_id))
            .invisible()
            .sizing_pass(),
    );
    add(&mut measure);
    let content_height = measure.min_rect().height().max(min_content_height);
    let overflow = if title == "Procesos" { 5. } else { 0. };
    let height = 16. + 24. + 10. + content_height + 16. - overflow;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), egui::Sense::hover());
    paint_glass_surface(ui, rect, 22.);

    let inner = rect.shrink(16.);
    let mut content = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::top_down(egui::Align::Min))
            .id_salt(("glass-content", panel_id)),
    );
    content.horizontal(|ui| {
        ui.label(
            egui::RichText::new(title)
                .color(secondary())
                .size(17.)
                .strong(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            icon(ui, kind, color, 16.);
        });
    });
    content.add_space(10.);
    add(&mut content);
}

fn paint_glass_surface(ui: &egui::Ui, rect: Rect, rounding: f32) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, rounding, glass());
    painter.rect_stroke(
        rect,
        rounding,
        Stroke::new(1.2, glass_border()),
        egui::StrokeKind::Inside,
    );
    painter.rect_stroke(
        rect.shrink(2.),
        (rounding - 2.).max(0.),
        Stroke::new(0.8, with_alpha(secondary(), 26)),
        egui::StrokeKind::Inside,
    );
    painter.line_segment(
        [
            Pos2::new(rect.left() + rounding, rect.top() + 1.),
            Pos2::new(rect.right() - rounding, rect.top() + 1.),
        ],
        Stroke::new(1., with_alpha(secondary(), 45)),
    );
}

fn process_table_header(ui: &mut egui::Ui, widths: &[f32], labels: &[&str], right: &[usize]) {
    let width: f32 = widths.iter().sum();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 34.), egui::Sense::hover());
    let mut x = rect.left();
    for (index, (label, cell_width)) in labels.iter().zip(widths).enumerate() {
        let cell = Rect::from_min_size(
            Pos2::new(x + 8., rect.top()),
            Vec2::new((cell_width - 16.).max(1.), rect.height()),
        );
        let is_right = right.contains(&index);
        ui.painter().with_clip_rect(cell).text(
            Pos2::new(
                if is_right {
                    cell.right() - 4.
                } else {
                    cell.left() + 4.
                },
                cell.center().y,
            ),
            if is_right {
                egui::Align2::RIGHT_CENTER
            } else {
                egui::Align2::LEFT_CENTER
            },
            *label,
            egui::FontId::proportional(12.),
            muted(),
        );
        x += cell_width;
    }
    ui.painter().line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1., with_alpha(secondary(), 48)),
    );
}

fn icon_action_button(
    ui: &mut egui::Ui,
    label: &str,
    kind: Icon,
    color: Color32,
) -> egui::Response {
    let response = ui.add_sized(
        Vec2::new(218., 40.),
        egui::Button::new(
            egui::RichText::new(label)
                .size(13.)
                .strong()
                .color(primary()),
        )
        .fill(tertiary())
        .stroke(Stroke::new(1., with_alpha(tertiary(), 150)))
        .corner_radius(13.),
    );
    if response.hovered() {
        ui.painter()
            .rect_filled(response.rect, 13., with_alpha(tertiary(), 48));
    }
    let icon_rect = Rect::from_center_size(
        Pos2::new(response.rect.left() + 24., response.rect.center().y),
        Vec2::splat(17.),
    );
    paint_icon(ui, kind, color, icon_rect);
    response
}

fn process_table_row(
    ui: &mut egui::Ui,
    widths: &[f32],
    values: &[String],
    right: &[usize],
    selected: bool,
    height: f32,
) -> bool {
    let width: f32 = widths.iter().sum();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), egui::Sense::click());
    let fill = if selected {
        with_alpha(tertiary(), 30)
    } else if response.hovered() {
        with_alpha(secondary(), 24)
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 10., fill);
    if selected {
        ui.painter().rect_filled(
            Rect::from_min_max(rect.left_top(), Pos2::new(rect.left() + 3., rect.bottom())),
            1.5,
            teal(),
        );
    }
    let mut x = rect.left();
    for (index, (value, cell_width)) in values.iter().zip(widths).enumerate() {
        let cell = Rect::from_min_size(
            Pos2::new(x + 8., rect.top() + 3.),
            Vec2::new((cell_width - 16.).max(1.), rect.height() - 6.),
        );
        let is_right = right.contains(&index);
        let is_numeric = index == 0 || is_right;
        ui.painter().with_clip_rect(cell).text(
            Pos2::new(
                if is_right {
                    cell.right() - 5.
                } else {
                    cell.left() + 5.
                },
                cell.center().y,
            ),
            if is_right {
                egui::Align2::RIGHT_CENTER
            } else {
                egui::Align2::LEFT_CENTER
            },
            value,
            if is_numeric {
                egui::FontId::monospace(14.)
            } else {
                egui::FontId::proportional(14.)
            },
            if index == 0 {
                muted()
            } else if value.ends_with('%') {
                teal()
            } else {
                secondary()
            },
        );
        x += cell_width;
    }
    ui.painter().line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1., with_alpha(secondary(), 34)),
    );
    response.clicked()
}

fn sort_label(sort: ProcessSort) -> &'static str {
    match sort {
        ProcessSort::Cpu => "CPU total",
        ProcessSort::Memory => "Memoria",
        ProcessSort::Pid => "PID",
        ProcessSort::Name => "Programa",
        ProcessSort::Arguments => "Comando",
        ProcessSort::Threads => "Hilos",
        ProcessSort::Owner => "Usuario",
    }
}

fn process_program(process: &sysinfo::Process) -> String {
    let name = process.name().to_string_lossy().trim().to_owned();
    if !name.is_empty() {
        return name;
    }
    process
        .exe()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "proceso desconocido".into())
}

fn compact_bytes(bytes: u64) -> String {
    let mut value = bytes as f64;
    let units = ["B", "K", "M", "G", "T"];
    let mut unit = 0;
    while value >= 1024. && unit < units.len() - 1 {
        value /= 1024.;
        unit += 1;
    }
    if unit == 0 {
        format!("{value:.0}{}", units[unit])
    } else {
        format!("{value:.2}{}", units[unit])
    }
}

fn format_duration(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = (seconds / 3_600) % 24;
    let minutes = (seconds / 60) % 60;
    if days > 0 {
        format!("{days}d {hours:02}h {minutes:02}m")
    } else {
        format!("{hours:02}h {minutes:02}m")
    }
}

fn process_detail_chip(ui: &mut egui::Ui, width: f32, label: &str, value: String, color: Color32) {
    let size = Vec2::new(width.max(72.), 68.);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    let fill = if response.hovered() {
        with_alpha(tertiary(), 44)
    } else {
        with_alpha(secondary(), 28)
    };
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 16., fill);
    painter.rect_stroke(
        rect,
        16.,
        Stroke::new(1., with_alpha(secondary(), 48)),
        egui::StrokeKind::Inside,
    );
    painter.text(
        Pos2::new(rect.center().x, rect.center().y - 10.),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(11.),
        muted(),
    );
    painter.text(
        Pos2::new(rect.center().x, rect.center().y + 12.),
        egui::Align2::CENTER_CENTER,
        value,
        egui::FontId::proportional(17.),
        color,
    );
}

fn detail_section(ui: &mut egui::Ui, title: &str, value: &str) {
    ui.add_space(6.);
    ui.label(egui::RichText::new(title).color(muted()).size(12.));
    ui.label(
        egui::RichText::new(value)
            .monospace()
            .color(secondary())
            .size(12.),
    );
}
fn metric(ui: &mut egui::Ui, title: &str, value: String, pct: f32, color: Color32) {
    let icon_kind = match title {
        "CPU" => Icon::Cpu,
        _ => Icon::Memory,
    };
    panel_icon(ui, title, icon_kind, color, |ui| {
        ui.label(
            egui::RichText::new(value.clone())
                .size(23.)
                .strong()
                .color(secondary()),
        );
        if pct > 0. {
            ui.add(
                egui::ProgressBar::new((pct / 100.).clamp(0., 1.))
                    .fill(color)
                    .show_percentage(),
            );
        }
    });
}

fn memory_stat_panel(
    ui: &mut egui::Ui,
    title: &str,
    value: String,
    percent: f32,
    total: String,
    color: Color32,
) {
    panel_icon(ui, title, Icon::Memory, color, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(value.clone())
                    .size(22.)
                    .strong()
                    .color(secondary()),
            );
            ui.label(egui::RichText::new(format!("{percent:.0}%")).color(color));
        });
        ui.label(
            egui::RichText::new(format!("de {total} físicos"))
                .color(muted())
                .size(12.),
        );
        ui.add_space(4.);
        ui.add(
            egui::ProgressBar::new((percent / 100.).clamp(0., 1.))
                .fill(color)
                .desired_height(9.),
        );
        ui.add_space(10.);
    });
}

fn memory_donut(ui: &mut egui::Ui, fraction: f32, color: Color32, value: String, subtitle: String) {
    ui.horizontal(|ui| {
        donut(
            ui,
            fraction,
            color,
            format!("{:.0}%", fraction.clamp(0., 1.) * 100.),
        );
        ui.add_space(10.);
        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new(value.clone())
                    .size(17.)
                    .strong()
                    .color(secondary()),
            );
            ui.label(
                egui::RichText::new(subtitle.clone())
                    .size(11.)
                    .color(muted()),
            );
        });
    });
}

fn memory_cache_panel(ui: &mut egui::Ui, cached: u64, total: u64) {
    panel_icon(ui, "Caché", Icon::Memory, secondary(), |ui| {
        memory_donut(
            ui,
            cached as f32 / total.max(1) as f32,
            secondary(),
            bytes(cached),
            format!("de {} RAM", bytes(total)),
        );
    });
}

fn memory_swap_panel(ui: &mut egui::Ui, used: u64, total: u64) {
    panel_icon(ui, "Swap", Icon::Memory, secondary(), |ui| {
        memory_donut(
            ui,
            used as f32 / total.max(1) as f32,
            secondary(),
            bytes(used),
            format!("de {} swap", bytes(total)),
        );
    });
}

fn icon(ui: &mut egui::Ui, kind: Icon, color: Color32, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    paint_icon(ui, kind, color, rect);
}

fn paint_icon(ui: &egui::Ui, kind: Icon, color: Color32, rect: Rect) {
    let p = ui.painter();
    let size = rect.width();
    let stroke = Stroke::new((size / 11.).max(1.25), color);
    let c = rect.center();
    let l = rect.left() + size * 0.16;
    let r = rect.right() - size * 0.16;
    let t = rect.top() + size * 0.16;
    let b = rect.bottom() - size * 0.16;
    match kind {
        Icon::Dashboard => {
            for (x, y) in [
                (l, t),
                (c.x + size * 0.06, t),
                (l, c.y + size * 0.06),
                (c.x + size * 0.06, c.y + size * 0.06),
            ] {
                p.rect_stroke(
                    Rect::from_min_size(Pos2::new(x, y), Vec2::splat(size * 0.28)),
                    2.,
                    stroke,
                    egui::StrokeKind::Inside,
                );
            }
        }
        Icon::SidebarCollapse | Icon::SidebarExpand => {
            let left = l + size * 0.08;
            let right = r - size * 0.08;
            p.rect_stroke(
                Rect::from_min_max(Pos2::new(left, t), Pos2::new(right, b)),
                2.,
                stroke,
                egui::StrokeKind::Inside,
            );
            let divider = if matches!(kind, Icon::SidebarCollapse) {
                left + size * 0.32
            } else {
                right - size * 0.32
            };
            p.line_segment([Pos2::new(divider, t), Pos2::new(divider, b)], stroke);
            let (tip, direction) = if matches!(kind, Icon::SidebarCollapse) {
                (left + size * 0.19, 1.)
            } else {
                (right - size * 0.19, -1.)
            };
            p.line_segment(
                [
                    Pos2::new(tip - direction * size * 0.06, c.y - size * 0.12),
                    Pos2::new(tip, c.y),
                ],
                stroke,
            );
            p.line_segment(
                [
                    Pos2::new(tip, c.y),
                    Pos2::new(tip - direction * size * 0.06, c.y + size * 0.12),
                ],
                stroke,
            );
        }
        Icon::Cpu => {
            p.rect_stroke(
                Rect::from_center_size(c, Vec2::splat(size * 0.58)),
                3.,
                stroke,
                egui::StrokeKind::Inside,
            );
            p.rect_stroke(
                Rect::from_center_size(c, Vec2::splat(size * 0.23)),
                1.,
                stroke,
                egui::StrokeKind::Inside,
            );
            for x in [c.x - size * 0.2, c.x + size * 0.2] {
                p.line_segment([Pos2::new(x, t), Pos2::new(x, t - size * 0.10)], stroke);
                p.line_segment([Pos2::new(x, b), Pos2::new(x, b + size * 0.10)], stroke);
            }
        }
        Icon::Memory => {
            p.rect_stroke(
                Rect::from_min_max(Pos2::new(l, t + size * 0.20), Pos2::new(r, b - size * 0.05)),
                2.,
                stroke,
                egui::StrokeKind::Inside,
            );
            p.line_segment([Pos2::new(l, c.y), Pos2::new(r, c.y)], stroke);
            for x in [l + size * 0.18, c.x, r - size * 0.18] {
                p.line_segment(
                    [Pos2::new(x, b - size * 0.05), Pos2::new(x, b + size * 0.10)],
                    stroke,
                );
            }
        }
        Icon::Network => {
            p.circle_stroke(c, size * 0.34, stroke);
            p.line_segment([Pos2::new(l, c.y), Pos2::new(r, c.y)], stroke);
            p.line_segment([Pos2::new(c.x, t), Pos2::new(c.x, b)], stroke);
        }
        Icon::Processes => {
            p.rect_stroke(
                Rect::from_min_max(Pos2::new(l, t), Pos2::new(r, b)),
                2.,
                stroke,
                egui::StrokeKind::Inside,
            );
            for y in [t + size * 0.25, c.y, b - size * 0.25] {
                p.line_segment(
                    [Pos2::new(l + size * 0.22, y), Pos2::new(r - size * 0.14, y)],
                    stroke,
                );
            }
        }
        Icon::Battery => {
            p.rect_stroke(
                Rect::from_min_max(
                    Pos2::new(l, t + size * 0.24),
                    Pos2::new(r - size * 0.13, b - size * 0.24),
                ),
                2.,
                stroke,
                egui::StrokeKind::Inside,
            );
            p.line_segment(
                [
                    Pos2::new(r - size * 0.13, c.y - size * 0.10),
                    Pos2::new(r, c.y - size * 0.10),
                ],
                stroke,
            );
            p.line_segment(
                [
                    Pos2::new(r - size * 0.13, c.y + size * 0.10),
                    Pos2::new(r, c.y + size * 0.10),
                ],
                stroke,
            );
        }
        Icon::Download | Icon::Upload => {
            let up = matches!(kind, Icon::Upload);
            let (from, to) = if up {
                (b - size * 0.18, t + size * 0.18)
            } else {
                (t + size * 0.18, b - size * 0.18)
            };
            p.line_segment([Pos2::new(c.x, from), Pos2::new(c.x, to)], stroke);
            let d = if up { 1. } else { -1. };
            p.line_segment(
                [
                    Pos2::new(c.x, to),
                    Pos2::new(c.x - size * 0.16, to + d * size * 0.16),
                ],
                stroke,
            );
            p.line_segment(
                [
                    Pos2::new(c.x, to),
                    Pos2::new(c.x + size * 0.16, to + d * size * 0.16),
                ],
                stroke,
            );
        }
        Icon::Chart => {
            p.line_segment([Pos2::new(l, b), Pos2::new(l, t)], stroke);
            p.line_segment([Pos2::new(l, b), Pos2::new(r, b)], stroke);
            p.line_segment(
                [
                    Pos2::new(l + size * 0.14, b - size * 0.18),
                    Pos2::new(c.x, c.y + size * 0.04),
                ],
                stroke,
            );
            p.line_segment(
                [
                    Pos2::new(c.x, c.y + size * 0.04),
                    Pos2::new(r - size * 0.10, t + size * 0.14),
                ],
                stroke,
            );
        }
        Icon::Gpu => {
            p.rect_stroke(
                Rect::from_center_size(c, Vec2::new(size * 0.70, size * 0.46)),
                3.,
                stroke,
                egui::StrokeKind::Inside,
            );
            p.circle_stroke(c, size * 0.13, stroke);
            for y in [c.y - size * 0.14, c.y + size * 0.14] {
                p.line_segment([Pos2::new(l, y), Pos2::new(l - size * 0.10, y)], stroke);
            }
        }
        Icon::Temperature => {
            p.line_segment([Pos2::new(c.x, t), Pos2::new(c.x, b - size * 0.20)], stroke);
            p.circle_stroke(Pos2::new(c.x, b - size * 0.10), size * 0.16, stroke);
            p.line_segment(
                [
                    Pos2::new(c.x - size * 0.12, t),
                    Pos2::new(c.x + size * 0.12, t),
                ],
                stroke,
            );
        }
        Icon::Disk => {
            p.rect_stroke(
                Rect::from_center_size(c, Vec2::new(size * 0.72, size * 0.58)),
                3.,
                stroke,
                egui::StrokeKind::Inside,
            );
            p.line_segment(
                [
                    Pos2::new(l + size * 0.12, c.y),
                    Pos2::new(r - size * 0.12, c.y),
                ],
                stroke,
            );
            p.circle_filled(
                Pos2::new(r - size * 0.16, b - size * 0.16),
                size * 0.05,
                color,
            );
        }
    }
}

fn bytes(value: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = value as f64;
    let mut unit = 0;
    while value >= 1024. && unit < UNITS.len() - 1 {
        value /= 1024.;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}
fn average(values: &VecDeque<f32>) -> f32 {
    if values.is_empty() {
        0.
    } else {
        values.iter().sum::<f32>() / values.len() as f32
    }
}
fn chart_scaled(ui: &mut egui::Ui, data: &VecDeque<f32>, color: Color32) {
    let values: Vec<_> = data.iter().copied().collect();
    let max = values.iter().copied().fold(1., f32::max) * 1.08;
    draw_chart(
        ui,
        &[("Tráfico", &values, color)],
        max,
        1,
        ChartUnits::Bytes,
    );
}
fn donut(ui: &mut egui::Ui, fraction: f32, color: Color32, center: String) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(68.), egui::Sense::hover());
    let center_pos = rect.center();
    let radius = 25.;
    let painter = ui.painter();
    painter.circle_stroke(
        center_pos,
        radius,
        Stroke::new(6., Color32::from_rgba_unmultiplied(176, 162, 123, 30)),
    );
    let fraction = fraction.clamp(0., 1.);
    let points: Vec<_> = (0..=48)
        .filter_map(|i| {
            let t = i as f32 / 48.;
            if t > fraction {
                return None;
            }
            let angle = -std::f32::consts::FRAC_PI_2 + t * std::f32::consts::TAU;
            Some(Pos2::new(
                center_pos.x + radius * angle.cos(),
                center_pos.y + radius * angle.sin(),
            ))
        })
        .collect();
    if points.len() > 1 {
        painter.add(egui::Shape::line(points, Stroke::new(6., color)));
    }
    painter.text(
        center_pos,
        egui::Align2::CENTER_CENTER,
        center,
        egui::FontId::proportional(10.),
        Color32::WHITE,
    );
}
fn interface_addresses() -> Vec<String> {
    std::process::Command::new("ip")
        .args(["-o", "-4", "addr", "show", "scope", "global"])
        .output()
        .ok()
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter_map(|line| {
                    let parts: Vec<_> = line.split_whitespace().collect();
                    let inet = parts.iter().position(|p| *p == "inet")?;
                    let iface = *parts.get(1)?;
                    let kind = if iface.starts_with("tun")
                        || iface.starts_with("wg")
                        || iface.starts_with("vpn")
                    {
                        "VPN"
                    } else {
                        "Local"
                    };
                    Some(format!("{kind} · {iface} · {}", parts.get(inet + 1)?))
                })
                .collect()
        })
        .unwrap_or_default()
}
fn meminfo_value(key: &str) -> Option<u64> {
    std::fs::read_to_string("/proc/meminfo")
        .ok()?
        .lines()
        .find_map(|line| {
            let (label, value) = line.split_once(':')?;
            if label != key {
                return None;
            }
            value
                .split_whitespace()
                .next()?
                .parse::<u64>()
                .ok()
                .map(|kb| kb * 1024)
        })
}
fn ping_stats() -> (Option<f32>, Option<f32>) {
    let output = std::process::Command::new("ping")
        .args(["-n", "-c", "2", "-W", "1", "1.1.1.1"])
        .output();
    let text = output
        .ok()
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        .unwrap_or_default();
    let stats = text.lines().find(|line| line.contains("min/avg/max"));
    let Some(stats) = stats else {
        return (None, None);
    };
    let values = stats
        .split('=')
        .nth(1)
        .unwrap_or("")
        .trim()
        .split_whitespace()
        .next()
        .unwrap_or("");
    let nums: Vec<f32> = values
        .split('/')
        .filter_map(|value| value.parse().ok())
        .collect();
    (nums.get(1).copied(), nums.get(3).copied())
}

#[derive(Clone, Copy)]
enum ChartUnits {
    Percent,
    Bytes,
    Memory,
}

fn chart(ui: &mut egui::Ui, data: &VecDeque<f32>, color: Color32) {
    let values: Vec<_> = data.iter().copied().collect();
    let label = if color == tertiary() { "GPU" } else { "CPU" };
    draw_chart(ui, &[(label, &values, color)], 100., 1, ChartUnits::Percent);
}

fn history_chart(ui: &mut egui::Ui, data: &[(f32, f32)]) {
    let cpu: Vec<_> = data.iter().map(|sample| sample.0).collect();
    let ram: Vec<_> = data.iter().map(|sample| sample.1).collect();
    draw_chart(
        ui,
        &[("CPU", &cpu, secondary()), ("RAM", &ram, tertiary())],
        100.,
        60,
        ChartUnits::Percent,
    );
}

fn process_history_chart(ui: &mut egui::Ui, data: &VecDeque<(f32, f32)>, height: f32) {
    let cpu: Vec<_> = data.iter().map(|sample| sample.0).collect();
    let ram: Vec<_> = data.iter().map(|sample| sample.1).collect();
    draw_chart_with_height(
        ui,
        &[("CPU", &cpu, secondary()), ("RAM", &ram, tertiary())],
        100.,
        1,
        ChartUnits::Percent,
        height,
    );
}

fn memory_chart(ui: &mut egui::Ui, data: &VecDeque<(f32, f32)>, swap: bool) {
    let label = if swap { "Swap" } else { "RAM" };
    let color = if swap { tertiary() } else { secondary() };
    let values: Vec<_> = data
        .iter()
        .map(|sample| if swap { sample.1 } else { sample.0 })
        .collect();
    let max = values.iter().copied().fold(1., f32::max) * 1.08;
    draw_chart(ui, &[(label, &values, color)], max, 1, ChartUnits::Memory);
}

fn draw_chart(
    ui: &mut egui::Ui,
    series: &[(&str, &[f32], Color32)],
    y_max: f32,
    sample_interval_seconds: u64,
    units: ChartUnits,
) {
    let height = ui.available_height().clamp(220., 360.);
    draw_chart_with_height(ui, series, y_max, sample_interval_seconds, units, height);
}

fn draw_chart_with_height(
    ui: &mut egui::Ui,
    series: &[(&str, &[f32], Color32)],
    y_max: f32,
    sample_interval_seconds: u64,
    units: ChartUnits,
    height: f32,
) {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), height),
        egui::Sense::hover(),
    );
    if ui.is_sizing_pass() {
        return;
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 12., with_alpha(primary(), 108));

    let plot = Rect::from_min_max(
        Pos2::new(rect.left() + 48., rect.top() + 12.),
        Pos2::new(rect.right() - 12., rect.bottom() - 31.),
    );
    for tick in 0..=4 {
        let ratio = tick as f32 / 4.;
        let y = plot.bottom() - plot.height() * ratio;
        painter.line_segment(
            [Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)],
            Stroke::new(
                if tick == 0 { 1.2 } else { 0.8 },
                with_alpha(secondary(), if tick == 0 { 62 } else { 34 }),
            ),
        );
        let label = match units {
            ChartUnits::Percent => format!("{:.0}%", y_max * ratio),
            ChartUnits::Bytes => speed((y_max * ratio).max(0.) as u64),
            ChartUnits::Memory => format!("{:.1} GB", y_max * ratio),
        };
        painter.text(
            Pos2::new(plot.left() - 8., y),
            egui::Align2::RIGHT_CENTER,
            label,
            egui::FontId::proportional(10.),
            muted(),
        );
    }
    for tick in 0..=4 {
        let ratio = tick as f32 / 4.;
        let x = plot.left() + plot.width() * ratio;
        painter.line_segment(
            [Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())],
            Stroke::new(0.7, with_alpha(secondary(), 22)),
        );
    }

    let sample_count = series
        .iter()
        .map(|(_, values, _)| values.len())
        .max()
        .unwrap_or(0);
    let mut plotted = Vec::new();
    for (name, values, color) in series {
        if values.is_empty() {
            continue;
        }
        let points: Vec<_> = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let x_ratio = index as f32 / values.len().saturating_sub(1).max(1) as f32;
                let y_ratio = (value / y_max.max(f32::EPSILON)).clamp(0., 1.);
                Pos2::new(
                    plot.left() + plot.width() * x_ratio,
                    plot.bottom() - plot.height() * y_ratio,
                )
            })
            .collect();
        let fill = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 68);
        for pair in points.windows(2) {
            painter.add(egui::Shape::convex_polygon(
                vec![
                    pair[0],
                    pair[1],
                    Pos2::new(pair[1].x, plot.bottom()),
                    Pos2::new(pair[0].x, plot.bottom()),
                ],
                fill,
                Stroke::NONE,
            ));
        }
        painter.add(egui::Shape::line(points.clone(), Stroke::new(2., *color)));
        plotted.push((*name, *values, *color, points));
    }

    let total_seconds = sample_count.saturating_sub(1) as u64 * sample_interval_seconds;
    for (ratio, elapsed) in [(0., total_seconds), (0.5, total_seconds / 2), (1., 0)] {
        let x = plot.left() + plot.width() * ratio;
        let label = if elapsed == 0 {
            "Ahora".to_owned()
        } else if elapsed >= 60 {
            format!("−{} min", elapsed / 60)
        } else {
            format!("−{} s", elapsed)
        };
        painter.text(
            Pos2::new(x, plot.bottom() + 16.),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(10.),
            muted(),
        );
    }
    if plotted.len() > 1 {
        let legend_y = rect.top() + 3.;
        let mut legend_x = plot.left();
        for (name, _, color, _) in &plotted {
            painter.circle_filled(Pos2::new(legend_x + 4., legend_y + 7.), 3.5, *color);
            painter.text(
                Pos2::new(legend_x + 12., legend_y + 7.),
                egui::Align2::LEFT_CENTER,
                *name,
                egui::FontId::proportional(10.),
                muted(),
            );
            legend_x += 58.;
        }
    }

    if let Some(pointer) = response
        .hover_pos()
        .filter(|pointer| plot.contains(*pointer))
    {
        let x_ratio = ((pointer.x - plot.left()) / plot.width()).clamp(0., 1.);
        let x = plot.left() + plot.width() * x_ratio;
        painter.line_segment(
            [Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())],
            Stroke::new(1., Color32::from_white_alpha(145)),
        );
        let mut selected = Vec::new();
        for (name, values, color, points) in &plotted {
            if values.is_empty() {
                continue;
            }
            let index = (x_ratio * values.len().saturating_sub(1) as f32).round() as usize;
            if let (Some(value), Some(point)) = (values.get(index), points.get(index)) {
                painter.circle_filled(*point, 5., *color);
                painter.circle_stroke(*point, 7., Stroke::new(1., secondary()));
                selected.push((*name, *value, *color));
            }
        }
        let elapsed = total_seconds.saturating_sub((x_ratio * total_seconds as f32).round() as u64);
        response.on_hover_ui_at_pointer(|ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.label(
                    egui::RichText::new(if elapsed == 0 {
                        "Ahora".to_owned()
                    } else if elapsed >= 60 {
                        format!("Hace {} min", elapsed / 60)
                    } else {
                        format!("Hace {elapsed} s")
                    })
                    .strong(),
                );
                for (name, value, color) in selected {
                    let label = match units {
                        ChartUnits::Percent => format!("{value:.2}%"),
                        ChartUnits::Bytes => speed(value.max(0.) as u64),
                        ChartUnits::Memory => format!("{value:.2} GB"),
                    };
                    ui.colored_label(color, format!("{name}: {label}"));
                }
            });
        });
    }
}
fn speed(n: u64) -> String {
    let (v, u) = if n >= 1_048_576 {
        (n as f64 / 1_048_576., "MB/s")
    } else if n >= 1024 {
        (n as f64 / 1024., "KB/s")
    } else {
        (n as f64, "B/s")
    };
    format!("{v:.1} {u}")
}
fn clock_widget(ui: &mut egui::Ui) {
    let now = chrono::Local::now();
    let month = [
        "ENERO",
        "FEBRERO",
        "MARZO",
        "ABRIL",
        "MAYO",
        "JUNIO",
        "JULIO",
        "AGOSTO",
        "SEPTIEMBRE",
        "OCTUBRE",
        "NOVIEMBRE",
        "DICIEMBRE",
    ][now.month0() as usize];
    let weekday = match now.weekday() {
        chrono::Weekday::Mon => "Lunes",
        chrono::Weekday::Tue => "Martes",
        chrono::Weekday::Wed => "Miércoles",
        chrono::Weekday::Thu => "Jueves",
        chrono::Weekday::Fri => "Viernes",
        chrono::Weekday::Sat => "Sábado",
        chrono::Weekday::Sun => "Domingo",
    };
    let (rect, _) = ui.allocate_exact_size(Vec2::new(316., 66.), egui::Sense::hover());
    paint_glass_surface(ui, rect, 18.);
    let mut content = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink(14.))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    content.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.label(
            egui::RichText::new(now.format("%H:%M:%S").to_string())
                .monospace()
                .size(30.)
                .strong()
                .color(secondary()),
        );
        ui.add_space(20.);
        ui.separator();
        ui.add_space(12.);
        ui.allocate_ui_with_layout(
            Vec2::new(101., 38.),
            egui::Layout::top_down(egui::Align::Min).with_main_align(egui::Align::Center),
            |ui| {
                ui.add_space(2.);
                ui.label(
                    egui::RichText::new(format!("{} {month}", now.format("%d")))
                        .size(12.)
                        .strong()
                        .color(secondary()),
                );
                ui.label(egui::RichText::new(weekday).size(14.).color(muted()));
            },
        );
    });
}
fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id("sys-dashboard")
            .with_transparent(true)
            .with_inner_size([1100., 820.])
            .with_min_inner_size([760., 600.]),
        ..Default::default()
    };
    eframe::run_native(
        "Sys Dashboard",
        options,
        Box::new(|cc| Ok(Box::new(App::new(&cc.egui_ctx)))),
    )
}

fn battery_info() -> (Option<u8>, bool) {
    let Ok(entries) = std::fs::read_dir("/sys/class/power_supply") else {
        return (None, false);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let kind = std::fs::read_to_string(path.join("type")).unwrap_or_default();
        if kind.trim() != "Battery" {
            continue;
        }
        let capacity = std::fs::read_to_string(path.join("capacity"))
            .ok()
            .and_then(|v| v.trim().parse().ok());
        let status = std::fs::read_to_string(path.join("status")).unwrap_or_default();
        return (capacity, status.trim() == "Charging");
    }
    (None, false)
}
