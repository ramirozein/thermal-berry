use std::time::Instant;
use sysinfo::{MemoryRefreshKind, System, MINIMUM_CPU_UPDATE_INTERVAL};

pub struct SystemLoad {
    system: System,
    previous_cpu: Instant,
}

#[derive(Clone, Copy, Default)]
pub struct Load {
    // Outer Option distinguishes a short sampling interval from a failed read.
    pub cpu_percent: Option<Option<f32>>,
    pub used_bytes: u64,
    pub total_bytes: u64,
}

impl SystemLoad {
    pub fn new() -> Result<Self, sysinfo::Error> {
        let mut system = System::new()?;
        system.refresh_cpu_usage();
        Ok(Self {
            system,
            previous_cpu: Instant::now(),
        })
    }

    pub fn read(&mut self) -> Load {
        self.system
            .refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());
        let cpu_percent = (self.previous_cpu.elapsed() >= MINIMUM_CPU_UPDATE_INTERVAL).then(|| {
            self.system.refresh_cpu_usage();
            self.previous_cpu = Instant::now();
            (!self.system.cpus().is_empty())
                .then(|| self.system.global_cpu_usage())
                .filter(|usage| (0.0..=100.0).contains(usage))
        });
        Load {
            cpu_percent,
            used_bytes: self.system.used_memory(),
            total_bytes: self.system.total_memory(),
        }
    }
}
