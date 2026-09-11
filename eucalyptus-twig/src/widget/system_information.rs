use std::{
    fs::{self, File},
    io::{BufRead, BufReader},
    time::Duration,
};

use eucalyptus_cellulose::{Element, task::{Task, TaskExt}};
use heapless::HistoryBuf;
use iced_core::{Font, Point};
use iced_futures::Subscription;
use iced_widget::canvas::{LineCap, Stroke};
use serde::Deserialize;

use crate::{widget::{Widget, WidgetPadding}};

// TODO: Replace heapless::HistoryBuf with something that can be configured by user (runtime allocated)
const HISTORY_LEN: usize = 16;

pub struct SystemInformation {
    update: Duration,
    temperature_hardware_name: String,
    cpu_statistics: Option<CpuStatistics>,
    hwmon_was_here: Option<u64>,
    cpu_usage_history: HistoryBuf<f32, HISTORY_LEN>,
    memory_usage_history: HistoryBuf<f32, HISTORY_LEN>,
    temperature_history: HistoryBuf<f32, HISTORY_LEN>,
}

impl Widget for SystemInformation {
    type Config = Config;

    type Message = Message;

    fn new(config: &Self::Config) -> (Self, Task<Self::Message>) {
        (
            Self {
                update: Duration::from_secs_f64(config.update),
                temperature_hardware_name: config.temperature_hardware_name.clone(),
                cpu_statistics: None,
                hwmon_was_here: None,
                cpu_usage_history: HistoryBuf::new(),
                memory_usage_history: HistoryBuf::new(),
                temperature_history: HistoryBuf::new(),
            },
            Task::none(),
        )
    }

    fn update(&mut self, message: Self::Message) -> impl Into<Task<Self::Message>> {
        match message {
            Message::Update {
                cpu_statistics,
                memory_info,
                hardware_monitoring,
            } => {
                if let Some(cpu_statistics) = cpu_statistics
                    && let Some(old_cpu_statistics) =
                        self.cpu_statistics.replace(cpu_statistics.clone())
                {
                    let cpu_usage = 1.0 - cpu_statistics.idle_percentage(&old_cpu_statistics);
                    self.cpu_usage_history.write(cpu_usage as f32);
                }
                if let Some(memory_info) = memory_info {
                    let memory_usage = 1.0 - memory_info.available_percentage();
                    self.memory_usage_history.write(memory_usage as f32);
                }
                if let Some(hardware_monitoring) = hardware_monitoring {
                    self.hwmon_was_here = Some(hardware_monitoring.id);
                    let temperature = hardware_monitoring.average_temperature() as f64 / 1000.0;
                    self.temperature_history.write(temperature as f32);
                }
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Self::Message> {
        const CHART_WIDTH: u32 = 64;

        let thing = |chart, text| iced_widget::stack![iced_widget::container(text).center_x(CHART_WIDTH)].push_under(chart);

        iced_widget::row(
            [
                self.cpu_usage_history.recent().map(|cpu_usage| {
                    iced_widget::row![
                        iced_widget::text("\u{e322}")
                            .font(Font::with_name("Material Symbols Rounded")),
                        thing(
                            iced_widget::canvas(LineChart {
                                history: &self.cpu_usage_history,
                                scale: 1.0,
                            }),
                            iced_widget::text!("{:.0}%", (cpu_usage * 100.0).round()),
                        ),
                    ]
                    .into()
                }),
                self.memory_usage_history.recent().map(|memory_usage| {
                    iced_widget::row![
                        iced_widget::text("\u{f7a3}")
                            .font(Font::with_name("Material Symbols Rounded")),
                        thing(
                            iced_widget::canvas(LineChart {
                                history: &self.memory_usage_history,
                                scale: 1.0,
                            }),
                            iced_widget::text!("{:.0}%", (memory_usage * 100.0).round()),
                        ),
                    ]
                    .into()
                }),
                self.temperature_history.recent().map(|temperature| {
                    iced_widget::row![
                        iced_widget::text("\u{f076}")
                            .font(Font::with_name("Material Symbols Rounded")),
                        thing(
                            iced_widget::canvas(LineChart {
                                history: &self.temperature_history,
                                scale: 0.01,
                            }),
                            iced_widget::text!("{:.0}\u{b0}C", temperature.round()),
                        ),
                    ]
                    .into()
                }),
            ]
            .into_iter()
            .flatten(),
        )
        .spacing(4)
        .widget_padding()
        .into()
    }

    fn subscription(&self) -> impl Into<Subscription<Self::Message>> {
        Subscription::run_with((self.temperature_hardware_name.clone(), self.hwmon_was_here, self.update), |data| {
            let (temperature_hardware_name, hwmon_was_here, update) = data.clone();
            iced_runtime::task::sipper(async move |mut tx| {
                loop {
                    tx.send(Message::Update {
                        cpu_statistics: CpuStatistics::get(),
                        memory_info: MemoryInfo::get(),
                        hardware_monitoring: HardwareMonitoring::get(
                            &temperature_hardware_name,
                            hwmon_was_here,
                        ),
                    })
                    .await;
                    tokio::time::sleep(update).await;
                }
            })
        })
    }
}

struct LineChart<'a> {
    history: &'a HistoryBuf<f32, HISTORY_LEN>,
    scale: f32,
}

impl<Message> iced_widget::canvas::Program<Message> for LineChart<'_> {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced_renderer::Renderer,
        theme: &iced_widget::renderer::core::Theme,
        bounds: iced_core::Rectangle,
        _cursor: iced_core::mouse::Cursor,
    ) -> Vec<iced_widget::canvas::Geometry> {
        use iced_widget::canvas;

        let mut frame = canvas::Frame::new(renderer, bounds.size());

        let mut line = canvas::path::Builder::new();

        for (index, &record) in self.history.oldest_ordered().rev().enumerate() {
            let point = Point::new(
                frame.width() / (self.history.capacity() - 1) as f32 * (self.history.capacity() - 1 - index) as f32,
                frame.height() * (1.0 - record * self.scale)
            );
            if index == 0 {
                line.move_to(point);
            } else {
                line.line_to(point);
            }
        }

        frame.stroke(&line.build(), Stroke::default()
            .with_width(2.0)
            .with_color(theme.palette().text.scale_alpha(0.5))
            .with_line_cap(LineCap::Round));

        vec![frame.into_geometry()]
    }
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_update")]
    update: f64,
    #[serde(default = "default_temperature_hardware_name")]
    temperature_hardware_name: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            update: default_update(),
            temperature_hardware_name: default_temperature_hardware_name(),
        }
    }
}

fn default_update() -> f64 {
    1.0
}

fn default_temperature_hardware_name() -> String {
    "k10temp".to_owned()
}

#[expect(private_interfaces)]
pub enum Message {
    Update {
        cpu_statistics: Option<CpuStatistics>,
        memory_info: Option<MemoryInfo>,
        hardware_monitoring: Option<HardwareMonitoring>,
    },
}

// TODO: improve error message

struct MemoryInfo {
    // in kB
    pub total: u64,
    pub available: u64,
}

impl MemoryInfo {
    fn get() -> Option<Self> {
        let path = "/proc/meminfo";
        let file = BufReader::new(
            File::open(path)
                .map_err(|e| {
                    tracing::error!("Failed to open {path}: {e}");
                })
                .ok()?,
        );

        let mut total = None::<u64>;
        let mut available = None::<u64>;

        for line in file.lines() {
            if let Ok(line) = line
                && let &[name, value, unit] =
                    line.split_ascii_whitespace().collect::<Vec<_>>().as_slice()
                && let Ok(value) = value.parse()
            {
                let name = name.strip_suffix(":").unwrap_or(name);
                match name {
                    "MemTotal" => {
                        if unit != "kB" {
                            tracing::error!(
                                "MemTotal with unit `{unit}` is not supported yet, only support kB"
                            );
                            continue;
                        }

                        total = Some(value);

                        if total.is_some() && available.is_some() {
                            break;
                        }
                    }
                    "MemAvailable" => {
                        if unit != "kB" {
                            tracing::error!(
                                "MemAvailable with unit `{unit}` is not supported yet, only support kB"
                            );
                            continue;
                        }

                        available = Some(value);

                        if total.is_some() && available.is_some() {
                            break;
                        }
                    }
                    _ => (),
                }
            }
        }

        if let Some(total) = total
            && let Some(available) = available
        {
            Some(Self { total, available })
        } else {
            None
        }
    }
    fn available_percentage(&self) -> f64 {
        self.available as f64 / self.total as f64
    }
}

#[derive(Clone)]
struct CpuStatistics {
    user: u64,
    nice: u64,
    system: u64,
    idle: u64,
    iowait: u64,
    irq: u64,
    softirq: u64,
    steal: u64,
    #[allow(unused)]
    guest: u64,
    #[allow(unused)]
    guest_nice: u64,
}

impl CpuStatistics {
    fn get() -> Option<Self> {
        let path = "/proc/stat";
        let file = BufReader::new(
            File::open(path)
                .map_err(|e| {
                    tracing::error!("Failed to open {path}: {e}");
                })
                .ok()?,
        );

        file.lines().find_map(|line| {
            if let Ok(line) = line
                && let mut line = line.split_ascii_whitespace()
                && line.next() == Some("cpu")
                && let &[
                    Ok(user),
                    Ok(nice),
                    Ok(system),
                    Ok(idle),
                    Ok(iowait),
                    Ok(irq),
                    Ok(softirq),
                    Ok(steal),
                    Ok(guest),
                    Ok(guest_nice),
                ] = line.map(str::parse).collect::<Vec<_>>().as_slice()
            {
                Some(Self {
                    user,
                    nice,
                    system,
                    idle,
                    iowait,
                    irq,
                    softirq,
                    steal,
                    guest,
                    guest_nice,
                })
            } else {
                None
            }
        })
    }
    #[allow(dead_code)]
    fn busy_time(&self) -> u64 {
        let Self {
            user,
            nice,
            system,
            idle: _,
            iowait: _,
            irq,
            softirq,
            steal,
            guest: _,
            guest_nice: _,
        } = self;
        user + nice + system + irq + softirq + steal
    }
    fn total_time(&self) -> u64 {
        let Self {
            user,
            nice,
            system,
            idle,
            iowait,
            irq,
            softirq,
            steal,
            guest: _,
            guest_nice: _,
        } = self;
        user + nice + system + idle + iowait + irq + softirq + steal
    }
    #[allow(dead_code)]
    fn busy_percentage(&self, old: &Self) -> f64 {
        (self.busy_time() - old.busy_time()) as f64 / (self.total_time() - old.total_time()) as f64
    }
    fn idle_percentage(&self, old: &Self) -> f64 {
        (self.idle - old.idle) as f64 / (self.total_time() - old.total_time()) as f64
    }
}

struct HardwareMonitoring {
    id: u64,
    temperatures: Vec<i64>,
}

impl HardwareMonitoring {
    fn get(name: &str, was_here: Option<u64>) -> Option<Self> {
        was_here
            .and_then(|was_here| {
                fs::read_to_string(format!("/sys/class/hwmon/hwmon{was_here}/name"))
                    .is_ok_and(|x| x.trim() == name)
                    .then(|| {
                        let path = format!("/sys/class/hwmon/hwmon{was_here}/");
                        match fs::read_dir(&path) {
                            Ok(x) => Some((was_here, x)),
                            Err(e) => {
                                tracing::warn!("Failed to open directory `{path}`: {e}");
                                None
                            }
                        }
                    })
            })
            .flatten()
            .or_else(|| match fs::read_dir("/sys/class/hwmon") {
                Ok(hwmon_dir) => hwmon_dir.filter_map(Result::ok).find_map(|entry| {
                    entry
                        .file_name()
                        .to_str()
                        .and_then(|x| x.strip_prefix("hwmon"))
                        .and_then(|x| x.parse::<u64>().ok())
                        .and_then(|id| {
                            fs::read_to_string(entry.path().join("name"))
                                .is_ok_and(|x| x.trim() == name)
                                .then(|| entry.path().read_dir().ok())
                                .flatten()
                                .map(|dir| (id, dir))
                        })
                }),
                Err(e) => {
                    tracing::error!("Failed to open hwmon directory: {e}");
                    None
                }
            })
            .map(|(id, dir)| Self {
                id,
                temperatures: dir
                    .filter_map(|entry| {
                        entry.ok().and_then(|entry| {
                            entry
                                .file_name()
                                .to_str()
                                .is_some_and(|file_name| {
                                    file_name.starts_with("temp") && file_name.ends_with("input")
                                })
                                .then(|| {
                                    fs::read_to_string(entry.path())
                                        .ok()
                                        .and_then(|temp| temp.trim().parse().ok())
                                })
                                .flatten()
                        })
                    })
                    .collect(),
            })
    }
    fn average_temperature(&self) -> i64 {
        self.temperatures.iter().sum::<i64>() / self.temperatures.len() as i64
    }
}
