use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, MutexGuard};

use serde::Serialize;

use crate::config::Config;
use crate::curve::Hysteresis;
use crate::db::Db;
use crate::thermal::ThermalDevice;

/// Samples retained in the ring buffer (~2 min with the default 2s interval).
pub const HISTORY_CAPACITY: usize = 60;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TempReading {
    pub label: String,
    pub celsius: f32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FanReading {
    pub id: String,
    pub label: String,
    pub rpm: Option<u32>,
    /// Current boost expressed as 0-100.
    pub boost_percent: Option<u8>,
    pub max_rpm: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    pub timestamp_ms: u64,
    pub temps: Vec<TempReading>,
    pub fans: Vec<FanReading>,
}

pub struct AppState {
    pub inner: Mutex<Inner>,
}

pub struct Inner {
    pub device: Option<Box<dyn ThermalDevice>>,
    pub config: Config,
    pub db: Db,
    pub history: VecDeque<Sample>,
    /// Hysteresis state per fan id, alive only while mode == Curve.
    pub curve_states: HashMap<String, Hysteresis>,
}

impl AppState {
    pub fn new(device: Option<Box<dyn ThermalDevice>>, config: Config, db: Db) -> Self {
        Self {
            inner: Mutex::new(Inner {
                device,
                db,
                config,
                history: VecDeque::with_capacity(HISTORY_CAPACITY),
                curve_states: HashMap::new(),
            }),
        }
    }

    /// Locks the inner state, recovering from a poisoned mutex: a panic in
    /// one thread (e.g. the monitor) must not cascade into every command and
    /// the exit handler — worst case we see slightly stale data, and losing
    /// the fan restore on exit would be the real integrity problem.
    pub fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Inner {
    pub fn push_sample(&mut self, sample: Sample) {
        if self.history.len() >= HISTORY_CAPACITY {
            self.history.pop_front();
        }
        self.history.push_back(sample);
    }
}
