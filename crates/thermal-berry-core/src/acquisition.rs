use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread;
use std::time::{Duration, Instant};

use crate::config::FanMode;
use crate::gpu::{Gpu, GpuLoad};
use crate::load::{Load, SystemLoad};
use crate::observation::Observation;
use crate::state::Sample;

pub(crate) const SENSOR_TIMEOUT: Duration = Duration::from_millis(500);

struct Request<T> {
    deadline: Instant,
    reply: SyncSender<T>,
}

pub(crate) struct Probe<T>(SyncSender<Request<T>>);

impl<T: Send + 'static> Probe<T> {
    pub(crate) fn spawn<F>(initialize: impl FnOnce() -> F + Send + 'static) -> Self
    where
        F: FnMut() -> T,
    {
        let (sender, requests) = mpsc::sync_channel::<Request<T>>(1);
        thread::spawn(move || {
            let mut read = initialize();
            for request in requests {
                if Instant::now() < request.deadline {
                    let result = read();
                    if Instant::now() <= request.deadline {
                        let _ = request.reply.send(result);
                    }
                }
            }
        });
        Self(sender)
    }

    pub(crate) fn request(&self, deadline: Instant) -> Receiver<T> {
        let (reply, rx) = mpsc::sync_channel(1);
        let _ = self.0.try_send(Request { deadline, reply });
        rx
    }
}

pub(crate) struct LoadProbes {
    system: Probe<Load>,
    gpu: Probe<GpuLoad>,
}

impl LoadProbes {
    pub fn spawn() -> Self {
        Self {
            system: Probe::spawn(|| {
                let mut system = SystemLoad::new().ok();
                move || system.as_mut().map(SystemLoad::read).unwrap_or_default()
            }),
            gpu: Probe::spawn(|| {
                let gpu = Gpu::discover();
                move || gpu.as_ref().map(Gpu::read).unwrap_or_default()
            }),
        }
    }

    pub fn request(&self) -> PendingLoads {
        let deadline = Instant::now() + SENSOR_TIMEOUT;
        PendingLoads {
            system: self.system.request(deadline),
            gpu: self.gpu.request(deadline),
            deadline,
        }
    }
}

pub(crate) struct PendingLoads {
    system: Receiver<Load>,
    gpu: Receiver<GpuLoad>,
    deadline: Instant,
}

impl PendingLoads {
    // Called on the history worker, never while holding the control mutex.
    pub fn join(self, sample: &Sample, mode: Option<FanMode>) -> Observation {
        let remaining = || self.deadline.saturating_duration_since(Instant::now());
        let load = self.system.recv_timeout(remaining()).unwrap_or_default();
        let gpu = self.gpu.recv_timeout(remaining()).unwrap_or_default();
        let cpu_temp = sample
            .temps
            .iter()
            .filter(|t| {
                let label = t.label.to_ascii_lowercase();
                label == "cpu"
                    || label.starts_with("coretemp · package id")
                    || matches!(
                        label.as_str(),
                        "k10temp · tdie" | "k10temp · tctl" | "zenpower · tdie"
                    )
                    || label.starts_with("cpu_thermal ·")
            })
            .map(|t| f64::from(t.celsius))
            .reduce(f64::max);
        let mut observation = Observation {
            timestamp_ms: sample.timestamp_ms,
            cpu_temp_c: cpu_temp,
            gpu_temp_c: gpu.celsius.or_else(|| {
                if gpu.percent.is_some() {
                    return None;
                }
                let mut sensors = sample
                    .temps
                    .iter()
                    .filter(|t| t.label.eq_ignore_ascii_case("gpu"));
                let sensor = sensors.next()?;
                sensors
                    .next()
                    .is_none()
                    .then_some(f64::from(sensor.celsius))
            }),
            cpu_percent: load.cpu_percent.flatten().map(f64::from),
            gpu_percent: gpu.percent,
            fan_percent: None,
            rpm: None,
            fans: Vec::new(),
            mode,
            mem_percent: (load.total_bytes > 0 && load.used_bytes <= load.total_bytes)
                .then(|| 100.0 * load.used_bytes as f64 / load.total_bytes as f64),
            gpu_mem_percent: gpu.memory_percent,
            gpu_power_w: gpu.power_w,
        };
        observation.set_fans(&sample.fans);
        observation
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn late_gpu_reply_is_discarded_and_never_reused_for_a_new_tick() {
        let (signal, pause) = mpsc::channel();
        let (begun, started) = mpsc::channel();
        let probe = Probe::spawn(move || {
            let mut number = 0;
            move || {
                number += 1;
                if number == 1 {
                    begun.send(()).unwrap();
                    pause.recv().unwrap();
                }
                number
            }
        });
        let deadline = Instant::now() + Duration::from_millis(100);
        let first = probe.request(deadline);
        started.recv_timeout(Duration::from_secs(2)).unwrap();
        let _ = first.recv_timeout(deadline.saturating_duration_since(Instant::now()));
        signal.send(()).unwrap();
        assert!(first.recv_timeout(Duration::from_secs(2)).is_err());
        let next = probe.request(Instant::now() + Duration::from_secs(2));
        assert_eq!(next.recv_timeout(Duration::from_secs(2)).unwrap(), 2);
    }
}
