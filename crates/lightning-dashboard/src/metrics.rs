//! Collecte des métriques du PC (procfs, sysfs, hwmon, NVML).

use std::collections::VecDeque;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use nvml_wrapper::Nvml;
use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};
use tracing::{debug, warn};

use crate::config::DashboardConfig;

/// Photographie des métriques à un instant donné.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub hostname: String,
    pub time: DateTime<Local>,
    pub uptime: Duration,
    pub cpu: CpuMetrics,
    pub gpu: Option<GpuMetrics>,
    pub memory: MemoryMetrics,
    pub disks: Vec<DiskMetrics>,
    pub network: NetworkMetrics,
}

#[derive(Debug, Clone, Default)]
pub struct CpuMetrics {
    pub model: String,
    /// Charge globale, de 0 à 100.
    pub usage: f32,
    /// Charge par cœur logique, de 0 à 100.
    pub per_core: Vec<f32>,
    /// Fréquence moyenne des cœurs, en MHz.
    pub freq_mhz: Option<f32>,
    /// Fréquence du cœur le plus rapide, en MHz.
    pub freq_max_mhz: Option<f32>,
    pub temp_c: Option<f32>,
}

#[derive(Debug, Clone, Default)]
pub struct GpuMetrics {
    pub name: String,
    pub temp_c: Option<f32>,
    /// Charge du GPU, de 0 à 100.
    pub usage: Option<f32>,
    pub mem_used: u64,
    pub mem_total: u64,
    pub power_w: Option<f32>,
    pub power_limit_w: Option<f32>,
    /// Vitesse de chaque ventilateur, en pourcentage.
    pub fans: Vec<u32>,
    pub clock_mhz: Option<u32>,
    pub mem_clock_mhz: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MemoryMetrics {
    pub total: u64,
    pub used: u64,
    pub swap_total: u64,
    pub swap_used: u64,
}

#[derive(Debug, Clone, Default)]
pub struct DiskMetrics {
    pub mount: String,
    pub total: u64,
    pub used: u64,
}

#[derive(Debug, Clone, Default)]
pub struct NetworkMetrics {
    pub interface: String,
    /// Débit descendant, en octets par seconde.
    pub rx_bps: f64,
    /// Débit montant, en octets par seconde.
    pub tx_bps: f64,
}

/// Mini-historiques affichés en sparklines (valeur la plus récente en dernier).
#[derive(Debug, Clone, Default)]
pub struct History {
    pub capacity: usize,
    pub cpu_usage: VecDeque<f32>,
    pub cpu_temp: VecDeque<f32>,
    pub gpu_usage: VecDeque<f32>,
    pub gpu_temp: VecDeque<f32>,
    pub gpu_power: VecDeque<f32>,
    pub mem_usage: VecDeque<f32>,
    pub net_rx: VecDeque<f32>,
    pub net_tx: VecDeque<f32>,
}

impl History {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(2),
            ..Default::default()
        }
    }

    fn push(queue: &mut VecDeque<f32>, capacity: usize, value: f32) {
        if queue.len() >= capacity {
            queue.pop_front();
        }
        queue.push_back(value);
    }

    /// Ajoute les valeurs d'une photographie.
    pub fn record(&mut self, s: &Snapshot) {
        let cap = self.capacity;
        Self::push(&mut self.cpu_usage, cap, s.cpu.usage);
        Self::push(&mut self.cpu_temp, cap, s.cpu.temp_c.unwrap_or(0.0));
        if let Some(gpu) = &s.gpu {
            Self::push(&mut self.gpu_usage, cap, gpu.usage.unwrap_or(0.0));
            Self::push(&mut self.gpu_temp, cap, gpu.temp_c.unwrap_or(0.0));
            Self::push(&mut self.gpu_power, cap, gpu.power_w.unwrap_or(0.0));
        }
        let mem = if s.memory.total > 0 {
            s.memory.used as f32 / s.memory.total as f32 * 100.0
        } else {
            0.0
        };
        Self::push(&mut self.mem_usage, cap, mem);
        Self::push(&mut self.net_rx, cap, s.network.rx_bps as f32);
        Self::push(&mut self.net_tx, cap, s.network.tx_bps as f32);
    }
}

/// Temps CPU cumulés d'une ligne de `/proc/stat`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CpuTimes {
    pub busy: u64,
    pub total: u64,
}

impl CpuTimes {
    /// Charge (0 à 100) entre deux relevés.
    pub fn usage_since(&self, prev: &CpuTimes) -> f32 {
        let total = self.total.saturating_sub(prev.total);
        if total == 0 {
            return 0.0;
        }
        let busy = self.busy.saturating_sub(prev.busy);
        (busy as f32 / total as f32 * 100.0).clamp(0.0, 100.0)
    }
}

/// Analyse `/proc/stat` : renvoie la ligne globale puis une entrée par cœur.
pub fn parse_proc_stat(text: &str) -> (CpuTimes, Vec<CpuTimes>) {
    let mut global = CpuTimes::default();
    let mut cores = Vec::new();
    for line in text.lines() {
        let mut fields = line.split_ascii_whitespace();
        let Some(label) = fields.next() else { continue };
        if !label.starts_with("cpu") {
            continue;
        }
        let v: Vec<u64> = fields.filter_map(|f| f.parse().ok()).collect();
        if v.len() < 5 {
            continue;
        }
        // user nice system idle iowait irq softirq steal (guest déjà compté dans user)
        let total: u64 = v.iter().take(8).sum();
        let idle = v[3] + v[4];
        let times = CpuTimes {
            busy: total - idle,
            total,
        };
        if label == "cpu" {
            global = times;
        } else {
            cores.push(times);
        }
    }
    (global, cores)
}

/// Analyse `/proc/meminfo` (valeurs en octets).
pub fn parse_meminfo(text: &str) -> MemoryMetrics {
    let field = |name: &str| -> u64 {
        text.lines()
            .find_map(|l| l.strip_prefix(name)?.strip_prefix(':'))
            .and_then(|rest| rest.split_ascii_whitespace().next()?.parse::<u64>().ok())
            .unwrap_or(0)
            * 1024
    };
    let total = field("MemTotal");
    let swap_total = field("SwapTotal");
    MemoryMetrics {
        total,
        used: total.saturating_sub(field("MemAvailable")),
        swap_total,
        swap_used: swap_total.saturating_sub(field("SwapFree")),
    }
}

/// Analyse `/proc/net/dev` : `(interface, octets reçus, octets émis)`.
pub fn parse_net_dev(text: &str) -> Vec<(String, u64, u64)> {
    text.lines()
        .filter_map(|line| {
            let (name, rest) = line.split_once(':')?;
            let v: Vec<u64> = rest
                .split_ascii_whitespace()
                .filter_map(|f| f.parse().ok())
                .collect();
            (v.len() >= 9).then(|| (name.trim().to_owned(), v[0], v[8]))
        })
        .collect()
}

/// Raccourcit le nom commercial d'un processeur ou d'une carte graphique.
pub fn short_model(name: &str) -> String {
    let mut out = name.trim().to_owned();
    for prefix in ["AMD ", "Intel(R) ", "NVIDIA ", "GeForce "] {
        if let Some(rest) = out.strip_prefix(prefix) {
            out = rest.to_owned();
        }
    }
    if let Some(idx) = out.find("-Core Processor") {
        // « Ryzen 9 9950X3D 16-Core Processor » -> « Ryzen 9 9950X3D »
        let head = out[..idx].trim_end();
        out = head
            .rsplit_once(' ')
            .map_or(head, |(model, _)| model)
            .to_owned();
    }
    out.replace("(R)", "").replace("(TM)", "").trim().to_owned()
}

fn read(path: impl AsRef<Path>) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn read_number<T: std::str::FromStr>(path: impl AsRef<Path>) -> Option<T> {
    read(path)?.trim().parse().ok()
}

/// Cherche le capteur de température du processeur dans hwmon.
fn find_cpu_temp_sensor() -> Option<PathBuf> {
    let entries = std::fs::read_dir("/sys/class/hwmon").ok()?;
    let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    dirs.sort();
    for wanted in ["k10temp", "zenpower", "coretemp"] {
        for dir in &dirs {
            if read(dir.join("name")).is_some_and(|n| n.trim() == wanted) {
                let input = dir.join("temp1_input");
                if input.exists() {
                    return Some(input);
                }
            }
        }
    }
    None
}

fn statvfs(mount: &str) -> Option<DiskMetrics> {
    let path = CString::new(mount).ok()?;
    // SAFETY: `path` est une chaîne C valide et `st` une structure initialisée à zéro.
    let st = unsafe {
        let mut st: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(path.as_ptr(), &mut st) != 0 {
            return None;
        }
        st
    };
    let block = st.f_frsize as u64;
    let total = st.f_blocks as u64 * block;
    let free = st.f_bfree as u64 * block;
    Some(DiskMetrics {
        mount: mount.to_owned(),
        total,
        used: total.saturating_sub(free),
    })
}

/// Photographie factice et déterministe, pour les captures d'écran et les
/// démonstrations : machine « workstation », valeurs plausibles qui ondulent
/// avec `t` (en secondes).
pub fn demo_snapshot(t: f32) -> Snapshot {
    let wave = |period: f32, phase: f32| ((t / period + phase).sin() + 1.0) / 2.0;
    let gpu_load = 35.0 + 55.0 * wave(7.0, 0.0);
    let cpu_load = 12.0 + 38.0 * wave(5.0, 1.3);
    Snapshot {
        hostname: "workstation".into(),
        time: Local::now(),
        uptime: Duration::from_secs(2 * 86_400 + 5 * 3600 + t as u64),
        cpu: CpuMetrics {
            model: lightning_i18n::tr("CPU 16 cœurs").into(),
            usage: cpu_load,
            per_core: (0..32)
                .map(|i| (cpu_load * (0.4 + 1.2 * wave(3.0, i as f32 * 0.7))).clamp(0.0, 100.0))
                .collect(),
            freq_mhz: Some(3200.0 + 1400.0 * wave(5.0, 1.3)),
            freq_max_mhz: Some(5500.0),
            temp_c: Some(48.0 + 20.0 * wave(9.0, 1.0)),
        },
        gpu: Some(GpuMetrics {
            name: lightning_i18n::tr("Carte graphique").into(),
            temp_c: Some(45.0 + 25.0 * wave(11.0, 0.4)),
            usage: Some(gpu_load),
            mem_used: ((6.0 + 10.0 * wave(13.0, 0.2)) * 1_073_741_824.0) as u64,
            mem_total: 32 << 30,
            power_w: Some(90.0 + 3.6 * gpu_load),
            power_limit_w: Some(600.0),
            fans: vec![(25.0 + 35.0 * wave(11.0, 0.4)) as u32],
            clock_mhz: Some(2600),
            mem_clock_mhz: Some(14000),
        }),
        memory: MemoryMetrics {
            total: 64 << 30,
            used: ((22.0 + 4.0 * wave(17.0, 0.0)) * 1_073_741_824.0) as u64,
            swap_total: 16 << 30,
            swap_used: 1 << 30,
        },
        disks: vec![
            DiskMetrics {
                mount: "/".into(),
                total: 500 << 30,
                used: 180 << 30,
            },
            DiskMetrics {
                mount: "/home".into(),
                total: 2000 << 30,
                used: 760 << 30,
            },
        ],
        network: NetworkMetrics {
            interface: "eth0".into(),
            rx_bps: 2.0e6 + 14.0e6 * f64::from(wave(4.0, 0.5)),
            tx_bps: 0.2e6 + 1.6e6 * f64::from(wave(6.0, 2.0)),
        },
    }
}

/// Collecteur à état : conserve les relevés précédents pour calculer les taux.
pub struct Collector {
    config: DashboardConfig,
    hostname: String,
    cpu_model: String,
    cpu_temp_sensor: Option<PathBuf>,
    prev_cpu: Option<(CpuTimes, Vec<CpuTimes>)>,
    prev_net: Option<(Instant, u64, u64)>,
    nvml: Option<Nvml>,
    history: History,
    /// Mode démonstration : instant de départ des données factices.
    demo: Option<Instant>,
}

impl Collector {
    /// Collecteur de démonstration : aucune lecture du système, des données
    /// factices (voir [`demo_snapshot`]) et un historique déjà rempli.
    pub fn new_demo(config: DashboardConfig) -> Self {
        let mut history = History::new(config.history);
        let points = history.capacity as u32;
        for i in 0..points {
            history.record(&demo_snapshot(i as f32 * 0.5 - points as f32 * 0.5));
        }
        Self {
            config,
            hostname: String::new(),
            cpu_model: String::new(),
            cpu_temp_sensor: None,
            prev_cpu: None,
            prev_net: None,
            nvml: None,
            history,
            demo: Some(Instant::now()),
        }
    }

    pub fn new(config: DashboardConfig) -> Self {
        let nvml = match Nvml::init() {
            Ok(nvml) => Some(nvml),
            Err(e) => {
                warn!("NVML indisponible, panneau GPU vide : {e}");
                None
            }
        };
        let cpu_model = read("/proc/cpuinfo")
            .and_then(|s| {
                s.lines()
                    .find_map(|l| l.strip_prefix("model name"))
                    .and_then(|l| l.split_once(':'))
                    .map(|(_, v)| short_model(v))
            })
            .unwrap_or_else(|| "CPU".into());
        let cpu_temp_sensor = find_cpu_temp_sensor();
        debug!(?cpu_temp_sensor, cpu_model, "collecteur initialisé");
        Self {
            history: History::new(config.history),
            hostname: read("/proc/sys/kernel/hostname")
                .map(|s| s.trim().to_owned())
                .unwrap_or_default(),
            config,
            cpu_model,
            cpu_temp_sensor,
            prev_cpu: None,
            prev_net: None,
            nvml,
            demo: None,
        }
    }

    /// Historiques accumulés par [`Collector::sample`].
    pub fn history(&self) -> &History {
        &self.history
    }

    /// Relève toutes les métriques et alimente les historiques.
    pub fn sample(&mut self) -> Snapshot {
        if let Some(started) = self.demo {
            let snapshot = demo_snapshot(started.elapsed().as_secs_f32());
            self.history.record(&snapshot);
            return snapshot;
        }
        let snapshot = Snapshot {
            hostname: self.hostname.clone(),
            time: Local::now(),
            uptime: read("/proc/uptime")
                .and_then(|s| s.split_ascii_whitespace().next()?.parse::<f64>().ok())
                .map(Duration::from_secs_f64)
                .unwrap_or_default(),
            cpu: self.sample_cpu(),
            gpu: self.sample_gpu(),
            memory: read("/proc/meminfo")
                .map(|s| parse_meminfo(&s))
                .unwrap_or_default(),
            disks: self.sample_disks(),
            network: self.sample_network(),
        };
        self.history.record(&snapshot);
        snapshot
    }

    /// Points de montage configurés, sans les doublons d'un même système de
    /// fichiers (sous-volumes btrfs montés à plusieurs endroits, par exemple).
    fn sample_disks(&self) -> Vec<DiskMetrics> {
        let mut disks: Vec<DiskMetrics> = Vec::new();
        for disk in self.config.disks.iter().filter_map(|m| statvfs(m)) {
            if !disks
                .iter()
                .any(|d| d.total == disk.total && d.used == disk.used)
            {
                disks.push(disk);
            }
        }
        disks
    }

    fn sample_cpu(&mut self) -> CpuMetrics {
        let (global, cores) = read("/proc/stat")
            .map(|s| parse_proc_stat(&s))
            .unwrap_or_default();
        let (usage, per_core) = match &self.prev_cpu {
            Some((prev_global, prev_cores)) if prev_cores.len() == cores.len() => (
                global.usage_since(prev_global),
                cores
                    .iter()
                    .zip(prev_cores)
                    .map(|(now, prev)| now.usage_since(prev))
                    .collect(),
            ),
            _ => (0.0, vec![0.0; cores.len()]),
        };
        let freqs: Vec<f32> = (0..cores.len())
            .filter_map(|i| {
                read_number::<f32>(format!(
                    "/sys/devices/system/cpu/cpu{i}/cpufreq/scaling_cur_freq"
                ))
            })
            .map(|khz| khz / 1000.0)
            .collect();
        self.prev_cpu = Some((global, cores));
        CpuMetrics {
            model: self.cpu_model.clone(),
            usage,
            per_core,
            freq_mhz: (!freqs.is_empty()).then(|| freqs.iter().sum::<f32>() / freqs.len() as f32),
            freq_max_mhz: freqs.iter().copied().reduce(f32::max),
            temp_c: self
                .cpu_temp_sensor
                .as_ref()
                .and_then(read_number::<f32>)
                .map(|milli| milli / 1000.0),
        }
    }

    fn sample_gpu(&self) -> Option<GpuMetrics> {
        let device = self.nvml.as_ref()?.device_by_index(0).ok()?;
        let memory = device.memory_info().ok();
        let fans = (0..device.num_fans().unwrap_or(0))
            .filter_map(|i| device.fan_speed(i).ok())
            .collect();
        Some(GpuMetrics {
            name: device
                .name()
                .map(|n| short_model(&n))
                .unwrap_or_else(|_| "GPU".into()),
            temp_c: device
                .temperature(TemperatureSensor::Gpu)
                .ok()
                .map(|t| t as f32),
            usage: device.utilization_rates().ok().map(|u| u.gpu as f32),
            mem_used: memory.as_ref().map_or(0, |m| m.used),
            mem_total: memory.as_ref().map_or(0, |m| m.total),
            power_w: device.power_usage().ok().map(|mw| mw as f32 / 1000.0),
            power_limit_w: device
                .enforced_power_limit()
                .ok()
                .map(|mw| mw as f32 / 1000.0),
            fans,
            clock_mhz: device.clock_info(Clock::Graphics).ok(),
            mem_clock_mhz: device.clock_info(Clock::Memory).ok(),
        })
    }

    fn sample_network(&mut self) -> NetworkMetrics {
        let all = read("/proc/net/dev")
            .map(|s| parse_net_dev(&s))
            .unwrap_or_default();
        let selected: Vec<&(String, u64, u64)> = match &self.config.interface {
            Some(name) => all.iter().filter(|(n, ..)| n == name).collect(),
            // Interfaces physiques : celles adossées à un périphérique.
            None => all
                .iter()
                .filter(|(n, ..)| Path::new("/sys/class/net").join(n).join("device").exists())
                .collect(),
        };
        let rx: u64 = selected.iter().map(|(_, rx, _)| rx).sum();
        let tx: u64 = selected.iter().map(|(_, _, tx)| tx).sum();
        let interface = selected
            .iter()
            .max_by_key(|(_, rx, tx)| rx + tx)
            .map(|(n, ..)| n.clone())
            .unwrap_or_default();
        let now = Instant::now();
        let (rx_bps, tx_bps) = match self.prev_net {
            Some((then, prev_rx, prev_tx)) => {
                let dt = now.duration_since(then).as_secs_f64().max(1e-3);
                (
                    rx.saturating_sub(prev_rx) as f64 / dt,
                    tx.saturating_sub(prev_tx) as f64 / dt,
                )
            }
            None => (0.0, 0.0),
        };
        self.prev_net = Some((now, rx, tx));
        NetworkMetrics {
            interface,
            rx_bps,
            tx_bps,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_stat_usage_between_two_samples() {
        let a = "cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 60 0 20 400 20 0 0 0 0 0\n\
                 cpu1 40 0 30 400 30 0 0 0 0 0\nintr 12345\n";
        let b = "cpu  200 0 100 850 50 0 0 0 0 0\ncpu0 160 0 20 400 20 0 0 0 0 0\n\
                 cpu1 40 0 80 450 30 0 0 0 0 0\n";
        let (ga, ca) = parse_proc_stat(a);
        let (gb, cb) = parse_proc_stat(b);
        assert_eq!(ca.len(), 2);
        assert_eq!(
            ga,
            CpuTimes {
                busy: 150,
                total: 1000
            }
        );
        assert!((gb.usage_since(&ga) - 75.0).abs() < 0.01);
        assert!((cb[0].usage_since(&ca[0]) - 100.0).abs() < 0.01);
        assert!((cb[1].usage_since(&ca[1]) - 50.0).abs() < 0.01);
        assert_eq!(ga.usage_since(&ga), 0.0);
    }

    #[test]
    fn meminfo_in_bytes() {
        let m = parse_meminfo(
            "MemTotal:       65536000 kB\nMemFree:         1000000 kB\n\
             MemAvailable:   49152000 kB\nSwapTotal:       8388608 kB\nSwapFree:        8388608 kB\n",
        );
        assert_eq!(m.total, 65_536_000 * 1024);
        assert_eq!(m.used, 16_384_000 * 1024);
        assert_eq!(m.swap_used, 0);
    }

    #[test]
    fn net_dev_counters() {
        let text = "Inter-|   Receive                                                |  Transmit\n \
            face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n    \
            lo: 1000 10 0 0 0 0 0 0 1000 10 0 0 0 0 0 0\n\
            eth0: 123456 100 0 0 0 0 0 0 654321 90 0 0 0 0 0 0\n";
        let v = parse_net_dev(text);
        assert_eq!(v.len(), 2);
        assert_eq!(v[1], ("eth0".to_owned(), 123456, 654321));
    }

    #[test]
    fn model_names_are_shortened() {
        assert_eq!(
            short_model(" AMD Ryzen 9 9950X3D 16-Core Processor"),
            "Ryzen 9 9950X3D"
        );
        assert_eq!(short_model("NVIDIA GeForce RTX 5090"), "RTX 5090");
        assert_eq!(short_model("Intel(R) Core(TM) i9-14900K"), "Core i9-14900K");
    }

    #[test]
    fn demo_collector_uses_generic_deterministic_data() {
        let a = demo_snapshot(12.5);
        let b = demo_snapshot(12.5);
        assert_eq!(a.hostname, "workstation");
        assert_eq!(a.network.interface, "eth0");
        assert_eq!(a.cpu.usage, b.cpu.usage);
        assert_eq!(a.cpu.per_core.len(), 32);
        assert!(a.cpu.per_core.iter().all(|u| (0.0..=100.0).contains(u)));
        assert_ne!(demo_snapshot(0.0).cpu.usage, demo_snapshot(3.0).cpu.usage);

        let mut collector = Collector::new_demo(DashboardConfig::default());
        assert_eq!(
            collector.history().cpu_usage.len(),
            120,
            "historique pré-rempli"
        );
        let snapshot = collector.sample();
        assert_eq!(snapshot.hostname, "workstation");
        assert!(snapshot.gpu.is_some());
        assert_eq!(collector.history().cpu_usage.len(), 120);
    }

    #[test]
    fn history_is_bounded() {
        let mut h = History::new(3);
        for i in 0..10 {
            History::push(&mut h.cpu_usage, h.capacity, i as f32);
        }
        assert_eq!(h.cpu_usage, [7.0, 8.0, 9.0]);
    }
}
