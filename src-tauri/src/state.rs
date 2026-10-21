use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, MutexGuard};

use serde::Serialize;

use crate::automatic::AutomaticController;
use crate::config::Config;
use crate::curve::Hysteresis;
use crate::db::Db;
use crate::thermal::ThermalDevice;

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
    pub automatic_states: HashMap<String, AutomaticController>,
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
                automatic_states: HashMap::new(),
            }),
        }
    }

    pub fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
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
