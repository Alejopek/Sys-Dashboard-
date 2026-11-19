use eframe::egui::{self, Color32, Pos2, Rect, Stroke, TextureHandle, Vec2};
use rusqlite::{Connection, params};
use std::{
    collections::VecDeque,
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};
use sysinfo::{Components, Disks, Gpus, Networks, System, Users};

const GLASS: Color32 = Color32::from_rgba_unmultiplied_const(10, 15, 23, 154);
const GLASS_STRONG: Color32 = Color32::from_rgba_unmultiplied_const(10, 15, 23, 198);
const GLASS_BORDER: Color32 = Color32::from_rgba_unmultiplied_const(225, 235, 245, 64);
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
        ui.columns(4, |cols| {
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
        panel_icon(ui, "Núcleos", Icon::Cpu, TEAL, |ui| {
            for (index, cpu) in self.system.cpus().iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.add_sized([100., 20.], egui::Label::new(format!("Núcleo {index}")));
                    ui.add(
                        egui::ProgressBar::new(cpu.usage() / 100.)
                            .fill(TEAL)
                            .text(format!("{:.1}%", cpu.usage())),
                    );
                    ui.add_sized(
                        [85., 20.],
                        egui::Label::new(format!("{} MHz", cpu.frequency())),
                    );
                });
            }
        });
        ui.add_space(12.);
        ui.columns(2, |cols| {
            panel_icon(
                &mut cols[0],
                "Temperaturas",
                Icon::Temperature,
                Color32::from_rgb(255, 184, 94),
                |ui| {
                    if self.components.is_empty() {
                        ui.label("No hay sensores disponibles.");
                    }
                    for sensor in self.components.iter() {
                        ui.horizontal(|ui| {
                            ui.label(sensor.label());
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        sensor
                                            .temperature()
                                            .map(|t| format!("{t:.1} °C"))
                                            .unwrap_or_else(|| "N/D".into()),
                                    );
                                },
                            );
                        });
                    }
                },
            );
            panel_icon(&mut cols[1], "GPU detectada", Icon::Gpu, PURPLE, |ui| {
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
        ui.columns(4, |cols| {
            metric(
                &mut cols[0],
                "Memoria usada",
                bytes(used),
                used as f32 * 100. / total.max(1) as f32,
                BLUE,
            );
            metric(
                &mut cols[1],
                "Memoria libre",
                bytes(free),
                free as f32 * 100. / total.max(1) as f32,
                TEAL,
            );
            panel_icon(&mut cols[2], "Caché estimada", Icon::Memory, PURPLE, |ui| {
                ui.label(bytes(cache));
            });
            panel_icon(
                &mut cols[3],
                "Swap",
                Icon::Memory,
                Color32::from_rgb(255, 184, 94),
                |ui| {
                    ui.label(format!(
                        "{} / {}",
                        bytes(self.system.used_swap()),
                        bytes(self.system.total_swap())
                    ));
                },
            );
        });
        ui.add_space(16.);
        panel_icon(ui, "Discos montados", Icon::Disk, BLUE, |ui| {
            table_header(
                ui,
                ["Punto de montaje", "Dispositivo", "Usado", "Libre", "Total"],
            );
            for disk in self.disks.iter() {
                let total = disk.total_space();
                let available = disk.available_space();
                table_row(
                    ui,
                    [
                        disk.mount_point().display().to_string(),
                        disk.name().to_string_lossy().into_owned(),
                        format!(
                            "{:.1}%",
                            (total - available) as f64 * 100. / total.max(1) as f64
                        ),
                        bytes(available),
                        bytes(total),
                    ],
                );
            }
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
        ui.add_space(16.);
        panel_icon(ui, "Interfaces e IP", Icon::Network, PURPLE, |ui| {
            ui.label(format!(
                "Total recibido: {} · Total enviado: {}",
                bytes(total_down),
                bytes(total_up)
            ));
            if self.interfaces.is_empty() {
                ui.label("No se pudieron obtener direcciones IP.");
            }
            for address in &self.interfaces {
                ui.monospace(address);
            }
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
                if ui
                    .selectable_label(self.process_sort == sort, label)
                    .clicked()
                {
                    if self.process_sort == sort {
                        self.process_descending = !self.process_descending;
                    } else {
                        self.process_sort = sort;
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
            ui.horizontal(|ui| {
                ui.add_sized(
                    [80., 26.],
                    egui::Label::new(egui::RichText::new("PID").color(MUTED)),
                );
                ui.add_sized(
                    [ui.available_width() * 0.38, 26.],
                    egui::Label::new(egui::RichText::new("PROCESO").color(MUTED)),
                );
                ui.add_sized(
                    [80., 26.],
                    egui::Label::new(egui::RichText::new("CPU").color(MUTED)),
                );
                ui.add_sized(
                    [100., 26.],
                    egui::Label::new(egui::RichText::new("RAM").color(MUTED)),
                );
                ui.label(egui::RichText::new("DUEÑO").color(MUTED));
            });
            ui.add_space(8.);
            for row in rows {
                let expanded = self.expanded_process == Some(row.pid);
                let mut response_clicked = false;
                egui::Frame::new()
                    .fill(if expanded {
                        Color32::from_rgba_unmultiplied(108, 221, 207, 28)
                    } else {
                        Color32::from_rgba_unmultiplied(255, 255, 255, 11)
                    })
                    .stroke(Stroke::new(
                        1.,
                        if expanded {
                            Color32::from_rgba_unmultiplied(108, 221, 207, 100)
                        } else {
                            GLASS_BORDER
                        },
                    ))
                    .corner_radius(10.)
                    .inner_margin(egui::Margin::symmetric(12, 9))
                    .show(ui, |ui| {
                        let (rect, response) = ui.allocate_exact_size(
                            Vec2::new(ui.available_width(), 32.),
                            egui::Sense::click(),
                        );
                        response_clicked = response.clicked();
                        let hover = response.hovered();
                        if hover {
                            ui.painter().rect_filled(
                                rect,
                                8.,
                                Color32::from_rgba_unmultiplied(255, 255, 255, 18),
                            );
                        }
                        let widths = [80., ui.available_width() * 0.38, 80., 100.];
                        let vals = [
                            row.pid.to_string(),
                            row.name.clone(),
                            format!("{:.1}%", row.cpu),
                            bytes(row.memory),
                            row.owner.clone(),
                        ];
                        let mut x = rect.left();
                        for (i, value) in vals.iter().enumerate() {
                            let w = if i < widths.len() {
                                widths[i]
                            } else {
                                (rect.right() - x).max(40.)
                            };
                            let color = if i == 2 {
                                TEAL
                            } else if i == 0 {
                                MUTED
                            } else {
                                Color32::WHITE
                            };
                            ui.painter().text(
                                Rect::from_min_size(
                                    Pos2::new(x, rect.top()),
                                    Vec2::new(w, rect.height()),
                                )
                                .center(),
                                egui::Align2::LEFT_CENTER,
                                value,
                                egui::FontId::proportional(15.),
                                color,
                            );
                            x += w;
                        }
                    });
                if response_clicked {
                    self.expanded_process = if expanded { None } else { Some(row.pid) };
                }
                let visible = self.expanded_process == Some(row.pid);
                let animation = ui.ctx().animate_bool_with_time(
                    egui::Id::new(("process", row.pid)),
                    visible,
                    0.18,
                );
                if animation > 0. {
                    egui::Frame::new()
                        .fill(Color32::from_rgba_unmultiplied(
                            6,
                            10,
                            17,
                            (animation * 170.) as u8,
                        ))
                        .corner_radius(8.)
                        .inner_margin(12.)
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
                                .button(
                                    egui::RichText::new("Finalizar proceso")
                                        .color(Color32::from_rgb(255, 140, 140)),
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
                ui.separator();
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
        paint_wallpaper(ui, &self.wallpaper);
        egui::Panel::left("sidebar")
            .size_range(210.0..=210.0)
            .frame(glass_frame(GLASS_STRONG, 16.))
            .show(ui, |ui| {
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
                            let width = ui.available_width();
                            let col_pid = width * 0.30;
                            let col_name = width * 0.55;
                            let col_value = width - col_pid - col_name;
                            egui::Frame::new()
                                .stroke(Stroke::new(1., Color32::from_gray(43)))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.add_sized(
                                            [col_pid, 24.],
                                            egui::Label::new(
                                                egui::RichText::new("PID").color(MUTED).size(10.),
                                            ),
                                        );
                                        ui.add_sized(
                                            [col_name, 24.],
                                            egui::Label::new(
                                                egui::RichText::new("PROCESO")
                                                    .color(MUTED)
                                                    .size(10.),
                                            ),
                                        );
                                        ui.add_sized(
                                            [col_value, 24.],
                                            egui::Label::new(
                                                egui::RichText::new(if self.memory_view {
                                                    "RAM %"
                                                } else {
                                                    "CPU %"
                                                })
                                                .color(MUTED)
                                                .size(10.),
                                            ),
                                        );
                                    });
                                });
                            let list = if self.memory_view {
                                &self.sample.top_mem
                            } else {
                                &self.sample.top_cpu
                            };
                            for (idx, p) in list.iter().enumerate() {
                                if idx > 0 {
                                    ui.separator();
                                }
                                ui.horizontal(|ui| {
                                    ui.add_sized(
                                        [col_pid, 23.],
                                        egui::Label::new(
                                            egui::RichText::new(p.pid.to_string())
                                                .monospace()
                                                .color(MUTED),
                                        ),
                                    );
                                    ui.add_sized([col_name, 23.], egui::Label::new(&p.name));
                                    ui.add_sized(
                                        [col_value, 23.],
                                        egui::Label::new(
                                            egui::RichText::new(format!(
                                                "{:.1}%",
                                                if self.memory_view { p.mem } else { p.cpu }
                                            ))
                                            .monospace()
                                            .color(TEAL)
                                            .strong(),
                                        ),
                                    );
                                });
                            }
                        });
                        if let Some(err) = &self.error {
                            ui.colored_label(Color32::LIGHT_RED, format!("SQLite: {err}"));
                        }
                    });
            });
    }

    /*fn processes_page(&mut self, ui: &mut egui::Ui) {
        Self::page_title(ui, "Procesos", "Todos los procesos activos; seleccioná una fila para ver los detalles");
        ui.horizontal(|ui| {
            ui.label("Filtrar");
            ui.add(egui::TextEdit::singleline(&mut self.process_filter).hint_text("Nombre, comando o propietario"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                for (sort, label) in [(ProcessSort::Owner, "Dueño"), (ProcessSort::Name, "Nombre"), (ProcessSort::Pid, "PID"), (ProcessSort::Memory, "RAM"), (ProcessSort::Cpu, "CPU")] {
                    if ui.selectable_label(self.process_sort == sort, label).clicked() {
                        if self.process_sort == sort { self.process_descending = !self.process_descending; } else { self.process_sort = sort; self.process_descending = matches!(sort, ProcessSort::Cpu | ProcessSort::Memory); }
                    }
                }
            });
        });
        let filter = self.process_filter.to_lowercase();
        let mut rows: Vec<_> = self.process_rows.iter().filter(|p| filter.is_empty() || p.name.to_lowercase().contains(&filter) || p.command.to_lowercase().contains(&filter) || p.owner.to_lowercase().contains(&filter)).cloned().collect();
        rows.sort_by(|a, b| { let order = match self.process_sort { ProcessSort::Cpu => a.cpu.total_cmp(&b.cpu), ProcessSort::Memory => a.memory.cmp(&b.memory), ProcessSort::Pid => a.pid.cmp(&b.pid), ProcessSort::Name => a.name.cmp(&b.name), ProcessSort::Owner => a.owner.cmp(&b.owner) }; if self.process_descending { order.reverse() } else { order } });
        ui.add_space(12.);
        panel_icon(ui, "Lista de procesos", Icon::Processes, BLUE, |ui| {
            table_header(ui, ["PID", "Proceso", "CPU", "RAM", "Dueño"]);
            for row in rows {
                let expanded = self.expanded_process == Some(row.pid);
                let response = ui.horizontal(|ui| {
                    ui.add_sized([90., 23.], egui::Label::new(row.pid.to_string()));
                    ui.add_sized([ui.available_width() * .36, 23.], egui::Label::new(&row.name));
                    ui.add_sized([80., 23.], egui::Label::new(format!("{:.1}%", row.cpu)));
                    ui.add_sized([100., 23.], egui::Label::new(bytes(row.memory)));
                    ui.label(&row.owner);
                }).response.interact(egui::Sense::click());
                if response.clicked() { self.expanded_process = if expanded { None } else { Some(row.pid) }; }
                let open = self.expanded_process == Some(row.pid);
                let t = ui.ctx().animate_bool_with_time(egui::Id::new(("process", row.pid)), open, 0.18);
                if t > 0. {
                    egui::Frame::new().fill(Color32::from_rgba_unmultiplied(6, 10, 17, (t * 170.) as u8)).corner_radius(8.).inner_margin(12.).show(ui, |ui| {
                        ui.label(egui::RichText::new("Comando").color(MUTED)); ui.monospace(if row.command.is_empty() { "No disponible" } else { &row.command });
                        ui.horizontal(|ui| { ui.label(format!("Estado: {}", row.status)); ui.separator(); ui.label(format!("Lectura: {}", bytes(row.read))); ui.separator(); ui.label(format!("Escritura: {}", bytes(row.written))); ui.separator(); ui.label(format!("Tiempo activo: {} s", row.runtime)); });
                        if ui.button(egui::RichText::new("Finalizar proceso").color(Color32::from_rgb(255, 140, 140))).clicked() {
                            if let Some(process) = self.system.process(sysinfo::Pid::from_u32(row.pid)) { if !process.kill() { self.error = Some(format!("No se pudo finalizar el proceso {}", row.pid)); } }
                        }
                    });
                }
                ui.separator();
            }
        });
    }*/
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
    add: impl FnOnce(&mut egui::Ui),
) {
    let frame = egui::Frame::new()
        .fill(GLASS)
        .stroke(Stroke::new(1., GLASS_BORDER))
        .corner_radius(16.)
        .inner_margin(16.);
    frame.show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(title)
                    .color(Color32::WHITE)
                    .size(16.)
                    .strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                icon(ui, kind, color, 15.);
            });
        });
        ui.add_space(10.);
        add(ui);
    });
}
fn metric(ui: &mut egui::Ui, title: &str, value: String, pct: f32, color: Color32) {
    let icon_kind = match title {
        "CPU" => Icon::Cpu,
        _ => Icon::Memory,
    };
    panel_icon(ui, title, icon_kind, color, |ui| {
        ui.label(
            egui::RichText::new(value)
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

fn load_wallpapers(ctx: &egui::Context) -> (TextureHandle, TextureHandle) {
    let source = image::load_from_memory(include_bytes!("../assets/wallpaper.jpg"))
        .expect("El wallpaper incluido debe ser un JPEG válido")
        .resize(1600, 1200, image::imageops::FilterType::Triangle);
    let sharp = source.to_rgba8();
    let blurred = source.blur(12.0).to_rgba8();
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

fn table_header(ui: &mut egui::Ui, labels: [&str; 5]) {
    ui.horizontal(|ui| {
        for label in labels {
            ui.add_sized(
                [ui.available_width() / 5., 22.],
                egui::Label::new(egui::RichText::new(label).color(MUTED).size(10.)),
            );
        }
    });
    ui.separator();
}
fn table_row(ui: &mut egui::Ui, values: [String; 5]) {
    ui.horizontal(|ui| {
        for value in values {
            ui.add_sized([ui.available_width() / 5., 22.], egui::Label::new(value));
        }
    });
    ui.separator();
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
