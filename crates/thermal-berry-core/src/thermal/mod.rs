pub mod alienware;
pub mod error;
mod hwmon;
mod thinkpad;

pub use error::ThermalError;

pub type Result<T> = std::result::Result<T, ThermalError>;

pub trait Fan: Send + Sync {
    fn id(&self) -> &str;
    fn label(&self) -> &str;
    fn read_rpm(&self) -> Result<u32>;
    fn read_boost(&self) -> Result<u8>;
    fn set_boost(&self, value: u8) -> Result<()>;
    fn boost_range(&self) -> (u8, u8);
    fn is_writable(&self) -> bool;
    fn max_rpm(&self) -> Option<u32>;
}

pub trait TempSensor: Send + Sync {
    fn label(&self) -> &str;
    fn read_celsius(&self) -> Result<f32>;
}

pub trait ThermalDevice: Send + Sync {
    fn vendor_name(&self) -> &str;
    fn driver_name(&self) -> &str;
    fn model(&self) -> Option<String>;
    fn fans(&self) -> Vec<Box<dyn Fan>>;
    fn sensors(&self) -> Vec<Box<dyn TempSensor>>;
    fn supports_auto_curve(&self) -> bool;
    fn supports_fan_control(&self) -> bool;
}

pub const VENDORS: &[&str] = &["alienware", thinkpad::VENDOR, "lenovo", "hp", "asus"];

pub fn configured_device(vendor: Option<&str>) -> Option<Box<dyn ThermalDevice>> {
    vendor
        .and_then(|v| create_device(v).ok())
        .or_else(|| detect_device().ok())
}

pub fn detect_device() -> Result<Box<dyn ThermalDevice>> {
    create_device("alienware").or_else(|_| Ok(Box::new(hwmon::HwmonDevice::detect(None)?)))
}

pub fn create_device(vendor: &str) -> Result<Box<dyn ThermalDevice>> {
    match vendor {
        "alienware" => Ok(Box::new(alienware::AlienwareDevice::detect()?)),
        candidate if hwmon::VENDORS.contains(&candidate) => {
            Ok(Box::new(hwmon::HwmonDevice::detect(Some(candidate))?))
        }
        other => Err(ThermalError::UnknownVendor(other.to_string())),
    }
}
