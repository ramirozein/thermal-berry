pub mod automatic;
pub mod config;
pub mod curve;
pub mod readings;

#[cfg(target_os = "linux")]
pub mod control;
#[cfg(target_os = "linux")]
pub mod db;
#[cfg(target_os = "linux")]
pub mod fan_control;
#[cfg(target_os = "linux")]
pub mod monitor;
#[cfg(target_os = "linux")]
pub mod state;
#[cfg(target_os = "linux")]
pub mod thermal;

#[cfg(target_os = "linux")]
mod acquisition;
#[cfg(target_os = "linux")]
pub mod calibration;
pub mod forecast;
#[cfg(target_os = "linux")]
mod gpu;
pub mod identify;
mod linalg;
pub mod load;
pub mod lock;
pub mod model;
pub mod observation;
mod storage;
