use eframe::egui::{self, Color32, Pos2, Rect, Stroke, TextureHandle, Vec2};
use rusqlite::{Connection, params};
use std::{
    collections::VecDeque,
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};
use sysinfo::{Components, Disks, Gpus, Networks, System, Users};

const GLASS: Color32 = Color32::from_rgba_unmultiplied_const(10, 18, 27, 142);
const GLASS_STRONG: Color32 = Color32::from_rgba_unmultiplied_const(10, 16, 23, 205);
const GLASS_BORDER: Color32 = Color32::from_rgba_unmultiplied_const(225, 240, 245, 96);
const MUTED: Color32 = Color32::from_rgb(194, 201, 213);
const TEAL: Color32 = Color32::from_rgb(108, 221, 207);
const BLUE: Color32 = Color32::from_rgb(111, 178, 255);
const PURPLE: Color32 = Color32::from_rgb(189, 149, 255);

#[derive(Clone)]
struct Proc {
    pid: u32,
    name: String,
    cpu: f32,
    mem: f32,
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
    Owner,
}
#[derive(Clone)]
struct ProcessRow {
    pid: u32,
    name: String,
    command: String,
    owner: String,
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
    top_cpu: Vec<Proc>,
    top_mem: Vec<Proc>,
}
struct App {
    wallpaper: TextureHandle,
    _blurred_wallpaper: TextureHandle,
    system: System,
    networks: Networks,
    disks: Disks,
    gpus: Gpus,
    components: Components,
    users: Users,
    db: Connection,
    sample: Sample,
    cpu_history: VecDeque<f32>,
    history: Vec<(f32, f32)>,
    last_tick: Instant,
    last_saved: Instant,
    memory_view: bool,
    error: Option<String>,
    page: Page,
    cpu_history_detail: VecDeque<f32>,
    gpu_history: VecDeque<f32>,
    download_history: VecDeque<f32>,
    upload_history: VecDeque<f32>,
    process_rows: Vec<ProcessRow>,
    process_sort: ProcessSort,
    process_descending: bool,
    process_filter: String,
    expanded_process: Option<u32>,
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
        ctx.set_style_of(egui::Theme::Dark, style);
        let (wallpaper, blurred_wallpaper) = load_wallpapers(ctx);
        GLASS_TEXTURE.with(|texture| *texture.borrow_mut() = Some(blurred_wallpaper.clone()));
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
            wallpaper,
            _blurred_wallpaper: blurred_wallpaper,
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
                top_cpu: vec![],
                top_mem: vec![],
            },
            cpu_history: VecDeque::new(),
            history,
            last_tick: Instant::now() - Duration::from_secs(2),
            last_saved: Instant::now(),
            memory_view: false,
            error: None,
            page: Page::General,
            cpu_history_detail: VecDeque::new(),
            gpu_history: VecDeque::new(),
            download_history: VecDeque::new(),
            upload_history: VecDeque::new(),
            process_rows: vec![],
            process_sort: ProcessSort::Cpu,
            process_descending: true,
            process_filter: String::new(),
            expanded_process: None,
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
        let total = self.system.total_memory() as f64 / 1_073_741_824.;
        let used = self.system.used_memory() as f64 / 1_073_741_824.;
        let totals = self.networks.iter().fold((0u64, 0u64), |a, (_, n)| {
            (a.0 + n.received(), a.1 + n.transmitted())
        });
        let down = totals.0;
        let up = totals.1;
        self.download_history.push_back(down as f32);
        self.upload_history.push_back(up as f32);
        let mut procs: Vec<Proc> = self
            .system
            .processes()
            .iter()
            .map(|(pid, p)| Proc {
                pid: pid.as_u32(),
                name: p.name().to_string_lossy().into_owned(),
                cpu: p.cpu_usage(),
                mem: if self.system.total_memory() > 0 {
                    p.memory() as f32 * 100. / self.system.total_memory() as f32
                } else {
                    0.
                },
            })
            .collect();
        self.process_rows = self
            .system
            .processes()
            .iter()
            .map(|(pid, p)| {
                let io = p.disk_usage();
                let owner = p
                    .user_id()
                    .and_then(|id| self.users.get_user_by_id(id))
                    .map(|u| u.name().to_owned())
                    .unwrap_or_else(|| "desconocido".into());
                ProcessRow {
                    pid: pid.as_u32(),
                    name: p.name().to_string_lossy().into_owned(),
                    command: p
                        .cmd()
                        .iter()
                        .map(|arg| arg.to_string_lossy())
                        .collect::<Vec<_>>()
                        .join(" "),
                    owner,
                    cpu: p.cpu_usage(),
                    memory: p.memory(),
                    status: format!("{:?}", p.status()),
                    read: io.read_bytes,
                    written: io.written_bytes,
                    runtime: p.run_time(),
                }
            })
            .collect();
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
        let mut top_cpu = procs.clone();
        top_cpu.sort_by(|a, b| b.cpu.total_cmp(&a.cpu));
        top_cpu.truncate(5);
        procs.sort_by(|a, b| b.mem.total_cmp(&a.mem));
        procs.truncate(5);
        self.sample = Sample {
            cpu,
            ram_used: used,
            ram_total: total,
            down,
            up,
            top_cpu,
            top_mem: procs,
        };
        self.cpu_history.push_back(cpu);
        self.cpu_history_detail.push_back(cpu);
        self.gpu_history.push_back(gpu_usage);
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
            .show(ui, |ui| match self.page {
                Page::Cpu => self.cpu_page(ui),
                Page::Memory => self.memory_page(ui),
                Page::Network => self.network_page(ui),
                Page::Processes => self.processes_page(ui),
                Page::General => {}
            });
    }

    fn page_title(ui: &mut egui::Ui, title: &str, subtitle: &str) {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.heading(
                    egui::RichText::new(title)
                        .color(Color32::WHITE)
                        .size(22.)
                        .strong(),
                );
                ui.label(egui::RichText::new(subtitle).color(MUTED).size(12.));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                glass_frame(GLASS_STRONG, 8.).show(ui, |ui| {
                    ui.monospace(chrono_like_time());
                });
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
            metric(
                &mut cols[0],
                "CPU total",
                format!("{:.1}%", self.sample.cpu),
                self.sample.cpu,
                TEAL,
            );
            panel_icon(&mut cols[1], "Carga media", Icon::Chart, PURPLE, |ui| {
                if let Some(load) = &load {
                    ui.label(format!(
                        "{:.2} · {:.2} · {:.2}",
                        load.one, load.five, load.fifteen
                    ));
                } else {
                    ui.label("No disponible");
                }
            });
            panel_icon(&mut cols[2], "Núcleos", Icon::Cpu, BLUE, |ui| {
                ui.label(format!(
                    "{} lógicos · {} físicos",
                    self.system.cpus().len(),
                    System::physical_core_count().unwrap_or(0)
                ));
            });
            panel_icon(&mut cols[3], "GPU", Icon::Gpu, PURPLE, |ui| {
                let usage = self.gpu_samples.first().and_then(|g| g.usage);
                ui.label(
                    usage
                        .map(|v| format!("{v:.1}%"))
                        .unwrap_or_else(|| "No disponible".into()),
                );
            });
            panel_icon(
                &mut cols[4],
                "Temperatura",
                Icon::Temperature,
                Color32::from_rgb(255, 184, 94),
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
            panel_icon(&mut cols[0], "Historial de CPU", Icon::Chart, TEAL, |ui| {
                chart(ui, &self.cpu_history_detail, TEAL)
            });
            panel_icon(&mut cols[1], "Historial de GPU", Icon::Gpu, PURPLE, |ui| {
                chart(ui, &self.gpu_history, PURPLE)
            });
        });
        ui.add_space(16.);
        ui.add_space(16.);
        panel_icon(ui, "Núcleos de CPU", Icon::Cpu, TEAL, |ui| {
            let tile_width = 132.;
            ui.horizontal_wrapped(|ui| {
                for (index, cpu) in self.system.cpus().iter().enumerate() {
                    ui.allocate_ui_with_layout(
                        Vec2::new(tile_width, 88.),
                        egui::Layout::top_down(egui::Align::Min),
                        |tile| {
                            egui::Frame::new()
                                .fill(Color32::from_rgba_unmultiplied(225, 245, 250, 34))
                                .stroke(Stroke::new(1., GLASS_BORDER))
                                .corner_radius(14.)
                                .inner_margin(egui::Margin::symmetric(10, 8))
                                .show(tile, |tile| {
                                    tile.horizontal(|tile| {
                                        tile.label(
                                            egui::RichText::new(format!("Core {index}"))
                                                .color(MUTED)
                                                .size(13.),
                                        );
                                        tile.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |tile| {
                                                tile.label(
                                                    egui::RichText::new(format!(
                                                        "{:.0}%",
                                                        cpu.usage()
                                                    ))
                                                    .color(Color32::WHITE)
                                                    .size(17.)
                                                    .strong(),
                                                );
                                            },
                                        );
                                    });
                                    tile.add_space(6.);
                                    tile.add(
                                        egui::ProgressBar::new(cpu.usage() / 100.)
                                            .fill(TEAL)
                                            .desired_height(9.),
                                    );
                                    tile.add_space(4.);
                                    tile.label(
                                        egui::RichText::new(format!("{} MHz", cpu.frequency()))
                                            .color(MUTED)
                                            .size(11.),
                                    );
                                });
                        },
                    );
                }
            });
        });
        ui.add_space(12.);
        panel_icon(ui, "GPU detectada", Icon::Gpu, PURPLE, |ui| {
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
                    BLUE,
                );
                memory_stat_panel(
                    &mut cols[1],
                    "Memoria libre",
                    bytes(free),
                    free as f32 * 100. / total.max(1) as f32,
                    total_label.clone(),
                    TEAL,
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
                                    BLUE,
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
                                    TEAL,
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
        panel_icon(ui, "Discos montados", Icon::Disk, BLUE, |ui| {
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
                                .fill(Color32::from_rgba_unmultiplied(225, 245, 250, 34))
                                .stroke(Stroke::new(1., GLASS_BORDER))
                                .corner_radius(14.)
                                .inner_margin(14.)
                                .show(card, |card| {
                                    card.label(
                                        egui::RichText::new(
                                            disk.mount_point().display().to_string(),
                                        )
                                        .size(15.)
                                        .strong()
                                        .color(Color32::WHITE),
                                    );
                                    card.label(
                                        egui::RichText::new(disk.name().to_string_lossy())
                                            .color(MUTED)
                                            .size(12.),
                                    );
                                    card.add_space(8.);
                                    card.add(
                                        egui::ProgressBar::new(pct)
                                            .fill(if pct > 0.9 {
                                                Color32::from_rgb(255, 151, 132)
                                            } else {
                                                BLUE
                                            })
                                            .text(format!("{:.0}% usado", pct * 100.))
                                            .desired_height(12.),
                                    );
                                    card.add_space(5.);
                                    card.horizontal(|card| {
                                        card.label(
                                            egui::RichText::new(format!("{} usados", bytes(used)))
                                                .size(11.),
                                        );
                                        card.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |card| {
                                                card.label(
                                                    egui::RichText::new(format!(
                                                        "{} libres",
                                                        bytes(available)
                                                    ))
                                                    .size(11.),
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
        panel_icon(ui, "Interfaces e IP", Icon::Network, PURPLE, |ui| {
            ui.horizontal_wrapped(|ui| {
                if self.interfaces.is_empty() {
                    ui.label("Sin IP detectada");
                }
                for address in &self.interfaces {
                    egui::Frame::new()
                        .fill(Color32::from_rgba_unmultiplied(255, 255, 255, 17))
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
            panel_icon(&mut cols[0], "Descarga", Icon::Download, TEAL, |ui| {
                ui.label(speed(self.sample.down));
                ui.label(format!(
                    "Máx. {} · Prom. {}",
                    speed(max_down as u64),
                    speed(avg_down as u64)
                ));
            });
            panel_icon(&mut cols[1], "Subida", Icon::Upload, BLUE, |ui| {
                ui.label(speed(self.sample.up));
                ui.label(format!(
                    "Máx. {} · Prom. {}",
                    speed(max_up as u64),
                    speed(avg_up as u64)
                ));
            });
            panel_icon(&mut cols[2], "Ping", Icon::Network, PURPLE, |ui| {
                ui.label(
                    self.ping_ms
                        .map(|v| format!("{v:.1} ms"))
                        .unwrap_or_else(|| "No disponible".into()),
                );
            });
            panel_icon(
                &mut cols[3],
                "Jitter",
                Icon::Network,
                Color32::from_rgb(255, 184, 94),
                |ui| {
                    ui.label(
                        self.jitter_ms
                            .map(|v| format!("{v:.1} ms"))
                            .unwrap_or_else(|| "No disponible".into()),
                    );
                },
            );
        });
        ui.add_space(16.);
        ui.columns(2, |cols| {
            panel_icon(&mut cols[0], "Descarga", Icon::Download, TEAL, |ui| {
                chart_scaled(ui, &self.download_history, TEAL)
            });
            panel_icon(&mut cols[1], "Subida", Icon::Upload, BLUE, |ui| {
                chart_scaled(ui, &self.upload_history, BLUE)
            });
        });
    }

    fn processes_page(&mut self, ui: &mut egui::Ui) {
        Self::page_title(
            ui,
            "Procesos",
            "Filtrá, ordená y expandí una fila para ver los detalles",
        );
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.process_filter)
                    .hint_text("Filtrar por nombre, comando o dueño"),
            );
            for (sort, label) in [
                (ProcessSort::Cpu, "CPU"),
                (ProcessSort::Memory, "RAM"),
                (ProcessSort::Pid, "PID"),
                (ProcessSort::Name, "Nombre"),
                (ProcessSort::Owner, "Dueño"),
            ] {
                let selected = self.process_sort == sort;
                if ui
                    .add(
                        egui::Button::new(egui::RichText::new(label).size(13.))
                            .selected(selected)
                            .fill(if selected {
                                Color32::from_rgba_unmultiplied(39, 160, 162, 118)
                            } else {
                                Color32::from_rgba_unmultiplied(220, 235, 240, 20)
                            })
                            .stroke(Stroke::new(
                                1.,
                                if selected {
                                    Color32::from_rgba_unmultiplied(108, 221, 207, 96)
                                } else {
                                    Color32::from_rgba_unmultiplied(225, 240, 245, 38)
                                },
                            ))
                            .corner_radius(14.)
                            .min_size(Vec2::new(50., 30.)),
                    )
                    .clicked()
                {
                    if self.process_sort == sort {
                        self.process_descending = !self.process_descending;
                    } else {
                        self.process_sort = sort;
                        self.process_descending =
                            matches!(sort, ProcessSort::Cpu | ProcessSort::Memory);
                    }
                }
            }
        });
        let filter = self.process_filter.to_lowercase();
        let mut rows: Vec<_> = self
            .process_rows
            .iter()
            .filter(|p| {
                filter.is_empty()
                    || p.name.to_lowercase().contains(&filter)
                    || p.command.to_lowercase().contains(&filter)
                    || p.owner.to_lowercase().contains(&filter)
            })
            .cloned()
            .collect();
        rows.sort_by(|a, b| {
            let sort = match self.process_sort {
                ProcessSort::Cpu => a.cpu.total_cmp(&b.cpu),
                ProcessSort::Memory => a.memory.cmp(&b.memory),
                ProcessSort::Pid => a.pid.cmp(&b.pid),
                ProcessSort::Name => a.name.cmp(&b.name),
                ProcessSort::Owner => a.owner.cmp(&b.owner),
            };
            if self.process_descending {
                sort.reverse()
            } else {
                sort
            }
        });
        ui.add_space(12.);
        panel_icon(ui, "Procesos activos", Icon::Processes, BLUE, |ui| {
            let table_width = ui.available_width();
            let widths = [
                table_width * 0.13,
                table_width * 0.39,
                table_width * 0.14,
                table_width * 0.17,
                table_width * 0.15,
            ];
            let headers = ["PID", "PROCESO", "CPU", "MEMORIA", "PROPIETARIO"];
            let (header_rect, _) =
                ui.allocate_exact_size(Vec2::new(table_width, 34.), egui::Sense::hover());
            let mut x = header_rect.left();
            for (index, (header, width)) in headers.iter().zip(widths).enumerate() {
                let cell = Rect::from_min_size(
                    Pos2::new(x, header_rect.top()),
                    Vec2::new(width, header_rect.height()),
                );
                ui.painter().text(
                    Pos2::new(
                        if index == 2 || index == 3 {
                            cell.right() - 14.
                        } else {
                            cell.left() + 14.
                        },
                        cell.center().y,
                    ),
                    if index == 2 || index == 3 {
                        egui::Align2::RIGHT_CENTER
                    } else {
                        egui::Align2::LEFT_CENTER
                    },
                    *header,
                    egui::FontId::proportional(13.),
                    MUTED,
                );
                x += width;
            }
            ui.painter().line_segment(
                [header_rect.left_bottom(), header_rect.right_bottom()],
                Stroke::new(1., Color32::from_rgba_unmultiplied(225, 240, 245, 42)),
            );
            ui.add_space(4.);
            for row in &rows {
                let expanded = self.expanded_process == Some(row.pid);
                let (rect, response) =
                    ui.allocate_exact_size(Vec2::new(table_width, 52.), egui::Sense::click());
                let fill = if expanded {
                    Color32::from_rgba_unmultiplied(92, 218, 200, 34)
                } else if response.hovered() {
                    Color32::from_rgba_unmultiplied(225, 245, 250, 24)
                } else {
                    Color32::TRANSPARENT
                };
                ui.painter().rect_filled(rect, 10., fill);
                if expanded {
                    ui.painter().rect_filled(
                        Rect::from_min_max(
                            rect.left_top(),
                            Pos2::new(rect.left() + 3., rect.bottom()),
                        ),
                        1.5,
                        TEAL,
                    );
                }
                let values = [
                    row.pid.to_string(),
                    row.name.clone(),
                    format!("{:.1} %", row.cpu),
                    bytes(row.memory),
                    row.owner.clone(),
                ];
                let mut x = rect.left();
                for (index, (value, width)) in values.iter().zip(widths).enumerate() {
                    let cell = Rect::from_min_size(
                        Pos2::new(x + 8., rect.top() + 3.),
                        Vec2::new((width - 16.).max(0.), rect.height() - 6.),
                    );
                    let painter = ui.painter().with_clip_rect(cell);
                    let numeric = index == 0 || index == 2 || index == 3;
                    let align = if index == 2 || index == 3 {
                        egui::Align2::RIGHT_CENTER
                    } else {
                        egui::Align2::LEFT_CENTER
                    };
                    let x_text = if index == 2 || index == 3 {
                        cell.right() - 5.
                    } else {
                        cell.left() + 5.
                    };
                    painter.text(
                        Pos2::new(x_text, cell.center().y),
                        align,
                        value,
                        if numeric {
                            egui::FontId::monospace(14.)
                        } else {
                            egui::FontId::proportional(14.)
                        },
                        match index {
                            0 => MUTED,
                            2 => TEAL,
                            _ => Color32::WHITE,
                        },
                    );
                    x += width;
                }
                if response.clicked() {
                    self.expanded_process = if expanded { None } else { Some(row.pid) };
                }
                ui.painter().line_segment(
                    [rect.left_bottom(), rect.right_bottom()],
                    Stroke::new(1., Color32::from_rgba_unmultiplied(225, 240, 245, 34)),
                );
                let visible = self.expanded_process == Some(row.pid);
                let animation = ui.ctx().animate_bool_with_time(
                    ui.scope_id().with(egui::IdSalt::new(("process", row.pid))),
                    visible,
                    0.18,
                );
                if animation > 0. {
                    egui::Frame::new()
                        .fill(Color32::from_rgba_unmultiplied(
                            7,
                            14,
                            21,
                            (animation * 176.) as u8,
                        ))
                        .stroke(Stroke::new(
                            1.,
                            Color32::from_rgba_unmultiplied(225, 240, 245, 54),
                        ))
                        .corner_radius(14.)
                        .inner_margin(egui::Margin::symmetric(16, 12))
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new("Comando").color(MUTED));
                            ui.monospace(if row.command.is_empty() {
                                "No disponible"
                            } else {
                                &row.command
                            });
                            ui.horizontal(|ui| {
                                ui.label(format!("Estado: {}", row.status));
                                ui.separator();
                                ui.label(format!("I/O lectura: {}", bytes(row.read)));
                                ui.separator();
                                ui.label(format!("I/O escritura: {}", bytes(row.written)));
                                ui.separator();
                                ui.label(format!("Activo: {} s", row.runtime));
                            });
                            if ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new("Terminar proceso")
                                            .color(Color32::from_rgb(255, 153, 153)),
                                    )
                                    .fill(Color32::from_rgba_unmultiplied(173, 54, 66, 54))
                                    .corner_radius(12.),
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
                                    }
                                }
                            }
                        });
                }
            }
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.last_tick.elapsed() >= Duration::from_secs(1) {
            self.refresh();
            self.last_tick = Instant::now();
        }
        ctx.request_repaint_after(Duration::from_millis(200));
        let viewport = ui.max_rect();
        paint_wallpaper(ui, &self.wallpaper);
        egui::Panel::left("sidebar")
            .size_range(210.0..=210.0)
            .frame(
                egui::Frame::new()
                    .fill(Color32::TRANSPARENT)
                    .stroke(Stroke::NONE)
                    .inner_margin(16.),
            )
            .show(ui, |ui| {
                paint_glass_surface(
                    ui,
                    Rect::from_min_size(viewport.min, Vec2::new(210., viewport.height())),
                    22.,
                );
                ui.add_space(12.);
                ui.horizontal(|ui| {
                    icon(ui, Icon::Dashboard, TEAL, 26.);
                    ui.label(
                        egui::RichText::new("SysDash")
                            .color(Color32::WHITE)
                            .size(21.)
                            .strong(),
                    );
                });
                ui.add_space(26.);
                ui.separator();
                ui.add_space(14.);
                ui.label(
                    egui::RichText::new("MONITOR")
                        .color(MUTED)
                        .size(10.)
                        .strong(),
                );
                ui.add_space(6.);
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
                                Color32::from_rgba_unmultiplied(74, 231, 206, 45)
                            } else {
                                Color32::from_rgba_unmultiplied(255, 255, 255, 20)
                            },
                        );
                    }
                    let icon_color = if selected { TEAL } else { MUTED };
                    let icon_rect =
                        Rect::from_min_size(rect.min + Vec2::new(11., 10.), Vec2::splat(17.));
                    paint_icon(ui, kind, icon_color, icon_rect);
                    ui.painter().text(
                        rect.min + Vec2::new(40., rect.height() * 0.5),
                        egui::Align2::LEFT_CENTER,
                        label,
                        egui::FontId::proportional(15.),
                        if selected { TEAL } else { MUTED },
                    );
                    if response
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        self.page = page;
                    }
                }
                ui.add_space(16.);
                ui.label(
                    egui::RichText::new("SISTEMA")
                        .color(MUTED)
                        .size(10.)
                        .strong(),
                );
                ui.add_space(8.);
                let (rect, processes) = ui.allocate_exact_size(
                    Vec2::new(ui.available_width(), 38.),
                    egui::Sense::click(),
                );
                if self.page == Page::Processes || processes.hovered() {
                    ui.painter().rect_filled(
                        rect,
                        10.,
                        if self.page == Page::Processes {
                            Color32::from_rgba_unmultiplied(74, 231, 206, 45)
                        } else {
                            Color32::from_rgba_unmultiplied(255, 255, 255, 20)
                        },
                    );
                }
                paint_icon(
                    ui,
                    Icon::Processes,
                    if self.page == Page::Processes {
                        TEAL
                    } else {
                        MUTED
                    },
                    Rect::from_min_size(rect.min + Vec2::new(11., 10.), Vec2::splat(17.)),
                );
                ui.painter().text(
                    rect.min + Vec2::new(40., rect.height() * 0.5),
                    egui::Align2::LEFT_CENTER,
                    "Procesos",
                    egui::FontId::proportional(15.),
                    MUTED,
                );
                if processes
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    self.page = Page::Processes;
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.horizontal(|ui| {
                        let p = ui.painter();
                        let c = ui.cursor().center();
                        p.circle_filled(c, 4., TEAL);
                        ui.add_space(10.);
                        ui.label(egui::RichText::new("Conectado").color(MUTED).size(11.));
                    });
                });
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(Color32::TRANSPARENT)
                    .inner_margin(24.),
            )
            .show(ui, |ui| {
                if self.page != Page::General {
                    self.render_page(ui);
                    return;
                }
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.heading(
                                    egui::RichText::new("Panel General")
                                        .color(Color32::from_rgb(224, 226, 233))
                                        .size(20.)
                                        .strong(),
                                );
                                ui.label(
                                    egui::RichText::new("Monitoreo en tiempo real")
                                        .color(MUTED)
                                        .size(11.),
                                );
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    glass_frame(GLASS_STRONG, 8.)
                                        .inner_margin(egui::Margin::symmetric(10, 6))
                                        .show(ui, |ui| {
                                            ui.label(
                                                egui::RichText::new(chrono_like_time())
                                                    .monospace()
                                                    .color(MUTED)
                                                    .size(11.),
                                            );
                                        });
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
                                TEAL,
                            );
                            metric(
                                &mut cols[1],
                                "RAM",
                                format!("{:.1} GB", self.sample.ram_used),
                                ram as f32,
                                BLUE,
                            );
                            panel_icon(&mut cols[2], "RED", Icon::Network, PURPLE, |ui| {
                                ui.horizontal(|ui| {
                                    icon(ui, Icon::Download, TEAL, 13.);
                                    ui.colored_label(TEAL, speed(self.sample.down));
                                });
                                ui.horizontal(|ui| {
                                    icon(ui, Icon::Upload, BLUE, 13.);
                                    ui.colored_label(BLUE, speed(self.sample.up));
                                });
                            });
                            let (battery, _charging) = battery_info();
                            panel_icon(
                                &mut cols[3],
                                "BATERÍA",
                                Icon::Battery,
                                Color32::from_rgb(255, 190, 87),
                                |ui| {
                                    if let Some(level) = battery {
                                        ui.label(
                                            egui::RichText::new(format!("{level}%"))
                                                .size(23.)
                                                .strong()
                                                .color(Color32::WHITE),
                                        );
                                        ui.add(
                                            egui::ProgressBar::new(level as f32 / 100.)
                                                .fill(Color32::from_rgb(255, 190, 87)),
                                        );
                                    } else {
                                        ui.label(
                                            egui::RichText::new("N/A")
                                                .size(23.)
                                                .strong()
                                                .color(MUTED),
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
                                    panel_icon(ui, "Uso de CPU", Icon::Cpu, TEAL, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new("Últimos 20 puntos")
                                                    .color(MUTED)
                                                    .size(10.),
                                            );
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    ui.colored_label(TEAL, "LIVE");
                                                },
                                            );
                                        });
                                        chart(ui, &self.cpu_history, TEAL);
                                    });
                                },
                            );
                            ui.allocate_ui_with_layout(
                                Vec2::new(width * 0.38 - 8., chart_row_height),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    panel_icon(ui, "Historial", Icon::Chart, PURPLE, |ui| {
                                        ui.label(
                                            egui::RichText::new("CPU & RAM promedio")
                                                .color(MUTED)
                                                .size(10.),
                                        );
                                        history_chart(ui, &self.history);
                                    });
                                },
                            );
                        });
                        ui.add_space(16.);
                        panel_icon(ui, "Procesos", Icon::Processes, BLUE, |ui| {
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.label(
                                        egui::RichText::new("Top 5 activos").color(MUTED).size(10.),
                                    );
                                });
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.selectable_value(&mut self.memory_view, true, "RAM");
                                        ui.selectable_value(&mut self.memory_view, false, "CPU");
                                    },
                                );
                            });
                            ui.add_space(8.);
                            let list = if self.memory_view {
                                &self.sample.top_mem
                            } else {
                                &self.sample.top_cpu
                            };
                            egui::Grid::new("general-process-cards")
                                .num_columns(5)
                                .spacing([8., 8.])
                                .show(ui, |ui| {
                                    for (index, p) in list.iter().enumerate() {
                                        let response = egui::Frame::new()
                                            .fill(Color32::from_rgba_unmultiplied(
                                                255, 255, 255, 14,
                                            ))
                                            .stroke(Stroke::new(1., GLASS_BORDER))
                                            .corner_radius(10.)
                                            .inner_margin(10.)
                                            .show(ui, |ui| {
                                                ui.set_min_width(150.);
                                                ui.label(
                                                    egui::RichText::new(&p.name)
                                                        .size(14.)
                                                        .strong()
                                                        .color(Color32::WHITE),
                                                );
                                                ui.label(
                                                    egui::RichText::new(format!("PID {}", p.pid))
                                                        .color(MUTED)
                                                        .size(11.),
                                                );
                                                ui.label(
                                                    egui::RichText::new(format!(
                                                        "{:.1}% {}",
                                                        if self.memory_view {
                                                            p.mem
                                                        } else {
                                                            p.cpu
                                                        },
                                                        if self.memory_view {
                                                            "RAM"
                                                        } else {
                                                            "CPU"
                                                        }
                                                    ))
                                                    .color(TEAL)
                                                    .size(13.)
                                                    .strong(),
                                                );
                                            });
                                        if response
                                            .response
                                            .interact(egui::Sense::click())
                                            .clicked()
                                        {
                                            self.page = Page::Processes;
                                        }
                                        if (index + 1) % 5 == 0 {
                                            ui.end_row();
                                        }
                                    }
                                });
                            if ui.button("Ver todos los procesos →").clicked() {
                                self.page = Page::Processes;
                            }
                        });
                        if let Some(err) = &self.error {
                            ui.colored_label(Color32::LIGHT_RED, format!("SQLite: {err}"));
                        }
                    });
            });
    }
}

#[derive(Clone, Copy)]
enum Icon {
    Dashboard,
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

thread_local! {
    static GLASS_TEXTURE: std::cell::RefCell<Option<TextureHandle>> = const { std::cell::RefCell::new(None) };
    static GLASS_VIEWPORT: std::cell::Cell<Rect> = const { std::cell::Cell::new(Rect::NOTHING) };
}

fn glass_frame(fill: Color32, padding: f32) -> egui::Frame {
    egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1., GLASS_BORDER))
        .corner_radius(16.)
        .inner_margin(padding)
}

fn panel_icon(
    ui: &mut egui::Ui,
    title: &str,
    kind: Icon,
    color: Color32,
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
    let content_height = measure.min_rect().height().max(0.);
    let height = 16. + 24. + 10. + content_height + 16.;
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
                .color(Color32::WHITE)
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
    let painter = ui.painter();
    painter.rect_filled(
        rect.expand(5.),
        rounding + 4.,
        Color32::from_rgba_unmultiplied(3, 15, 22, 72),
    );
    let viewport = GLASS_VIEWPORT.with(std::cell::Cell::get);
    let texture = GLASS_TEXTURE.with(|texture| texture.borrow().clone());
    if let Some(texture) = texture.filter(|_| viewport.width() > 0. && viewport.height() > 0.) {
        let mut mesh = egui::epaint::Mesh::with_texture(texture.id());
        let uv_at = |point: Pos2| {
            Pos2::new(
                ((point.x - viewport.left()) / viewport.width()).clamp(0., 1.),
                ((point.y - viewport.top()) / viewport.height()).clamp(0., 1.),
            )
        };
        let center_index = 0;
        mesh.vertices.push(egui::epaint::Vertex {
            pos: rect.center(),
            uv: uv_at(rect.center()),
            color: Color32::WHITE,
        });
        let radius = rounding.min(rect.width() * 0.5).min(rect.height() * 0.5);
        let corners = [
            (
                Pos2::new(rect.right() - radius, rect.top() + radius),
                -std::f32::consts::FRAC_PI_2,
            ),
            (Pos2::new(rect.right() - radius, rect.bottom() - radius), 0.),
            (
                Pos2::new(rect.left() + radius, rect.bottom() - radius),
                std::f32::consts::FRAC_PI_2,
            ),
            (
                Pos2::new(rect.left() + radius, rect.top() + radius),
                std::f32::consts::PI,
            ),
        ];
        for (corner, start_angle) in corners {
            for step in 0..=12 {
                let angle = start_angle + std::f32::consts::FRAC_PI_2 * step as f32 / 12.;
                let point = Pos2::new(
                    corner.x + radius * angle.cos(),
                    corner.y + radius * angle.sin(),
                );
                mesh.vertices.push(egui::epaint::Vertex {
                    pos: point,
                    uv: uv_at(point),
                    color: Color32::WHITE,
                });
            }
        }
        let ring_count = mesh.vertices.len() - 1;
        for index in 0..ring_count {
            let current = index as u32 + 1;
            let next = ((index + 1) % ring_count) as u32 + 1;
            mesh.add_triangle(center_index, current, next);
        }
        painter.add(egui::Shape::mesh(mesh));
    } else {
        painter.rect_filled(rect, rounding, GLASS);
    }
    painter.rect_filled(rect, rounding, GLASS);
    painter.rect_stroke(
        rect,
        rounding,
        Stroke::new(1.2, GLASS_BORDER),
        egui::StrokeKind::Inside,
    );
    painter.rect_stroke(
        rect.shrink(2.),
        (rounding - 2.).max(0.),
        Stroke::new(0.8, Color32::from_rgba_unmultiplied(255, 255, 255, 26)),
        egui::StrokeKind::Inside,
    );
    painter.line_segment(
        [
            Pos2::new(rect.left() + rounding, rect.top() + 1.),
            Pos2::new(rect.right() - rounding, rect.top() + 1.),
        ],
        Stroke::new(1., Color32::from_rgba_unmultiplied(255, 255, 255, 62)),
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
                .color(Color32::WHITE),
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
                    .color(Color32::WHITE),
            );
            ui.label(egui::RichText::new(format!("{percent:.0}%")).color(color));
        });
        ui.label(
            egui::RichText::new(format!("de {total} físicos"))
                .color(MUTED)
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
                    .color(Color32::WHITE),
            );
            ui.label(egui::RichText::new(subtitle.clone()).size(11.).color(MUTED));
        });
    });
}

fn memory_cache_panel(ui: &mut egui::Ui, cached: u64, total: u64) {
    panel_icon(ui, "Caché", Icon::Memory, PURPLE, |ui| {
        memory_donut(
            ui,
            cached as f32 / total.max(1) as f32,
            PURPLE,
            bytes(cached),
            format!("de {} RAM", bytes(total)),
        );
    });
}

fn memory_swap_panel(ui: &mut egui::Ui, used: u64, total: u64) {
    let orange = Color32::from_rgb(255, 184, 94);
    panel_icon(ui, "Swap", Icon::Memory, orange, |ui| {
        memory_donut(
            ui,
            used as f32 / total.max(1) as f32,
            orange,
            bytes(used),
            format!("de {} swap", bytes(total)),
        );
    });
}

fn load_wallpapers(ctx: &egui::Context) -> (TextureHandle, TextureHandle) {
    let source = image::load_from_memory(include_bytes!("../assets/wallpaper.jpg"))
        .expect("El wallpaper incluido debe ser un JPEG válido")
        .resize(1600, 1200, image::imageops::FilterType::Triangle);
    let sharp = source.to_rgba8();
    let blurred = source.blur(26.0).to_rgba8();
    let to_texture = |name: &str, image: image::RgbaImage| {
        let size = [image.width() as usize, image.height() as usize];
        let pixels = image.into_raw();
        ctx.load_texture(
            name,
            egui::ColorImage::from_rgba_unmultiplied(size, &pixels),
            egui::TextureOptions::LINEAR,
        )
    };
    (
        to_texture("sharp-wallpaper", sharp),
        to_texture("glass-blur-wallpaper", blurred),
    )
}

fn paint_wallpaper(ui: &egui::Ui, wallpaper: &TextureHandle) {
    let rect = ui.max_rect();
    GLASS_VIEWPORT.with(|viewport| viewport.set(rect));
    let painter = ui.painter();
    painter.image(
        wallpaper.id(),
        rect,
        Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
        Color32::WHITE,
    );
    painter.rect_filled(rect, 0., Color32::from_rgba_unmultiplied(5, 8, 14, 152));
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
    let max = data.iter().copied().fold(1., f32::max);
    let normalized: VecDeque<f32> = data.iter().map(|value| value * 100. / max).collect();
    chart(ui, &normalized, color);
}
fn donut(ui: &mut egui::Ui, fraction: f32, color: Color32, center: String) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(68.), egui::Sense::hover());
    let center_pos = rect.center();
    let radius = 25.;
    let painter = ui.painter();
    painter.circle_stroke(
        center_pos,
        radius,
        Stroke::new(6., Color32::from_rgba_unmultiplied(255, 255, 255, 30)),
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

fn chart(ui: &mut egui::Ui, data: &VecDeque<f32>, color: Color32) {
    let (r, _) = ui.allocate_exact_size(
        Vec2::new(
            ui.available_width(),
            ui.available_height().clamp(260., 480.),
        ),
        egui::Sense::hover(),
    );
    let p = ui.painter_at(r);
    p.rect_filled(r, 10., Color32::from_rgba_unmultiplied(4, 8, 16, 82));
    for i in 1..4 {
        let y = r.top() + r.height() * i as f32 / 4.;
        p.line_segment(
            [Pos2::new(r.left(), y), Pos2::new(r.right(), y)],
            Stroke::new(1., Color32::from_rgba_unmultiplied(255, 255, 255, 22)),
        );
    }
    if data.len() > 1 {
        let pts: Vec<_> = data
            .iter()
            .enumerate()
            .map(|(i, v)| {
                Pos2::new(
                    r.left() + r.width() * i as f32 / (data.len() - 1) as f32,
                    r.bottom() - r.height() * v.clamp(0., 100.) / 100.,
                )
            })
            .collect();
        p.add(egui::Shape::line(pts, Stroke::new(2., color)));
    }
}
fn history_chart(ui: &mut egui::Ui, data: &[(f32, f32)]) {
    let (r, _) = ui.allocate_exact_size(
        Vec2::new(
            ui.available_width(),
            ui.available_height().clamp(260., 480.),
        ),
        egui::Sense::hover(),
    );
    let p = ui.painter_at(r);
    p.rect_filled(r, 10., Color32::from_rgba_unmultiplied(4, 8, 16, 82));
    for (idx, color) in [(0, BLUE), (1, PURPLE)] {
        let pts: Vec<_> = data
            .iter()
            .enumerate()
            .map(|(i, v)| {
                Pos2::new(
                    r.left() + r.width() * i as f32 / (data.len().saturating_sub(1).max(1)) as f32,
                    r.bottom()
                        - r.height() * if idx == 0 { v.0 } else { v.1 }.clamp(0., 100.) / 100.,
                )
            })
            .collect();
        if pts.len() > 1 {
            p.add(egui::Shape::line(pts, Stroke::new(2., color)));
        }
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
fn chrono_like_time() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}
fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
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
