pub mod alienware;
pub mod error;

pub use error::ThermalError;

pub type Result<T> = std::result::Result<T, ThermalError>;

/// A single fan — knows nothing about temperature, only about itself.
pub trait Fan: Send + Sync {
    /// Stable identifier within the device, e.g. "fan1".
    fn id(&self) -> &str;
    /// Human-readable name, e.g. "CPU Fan".
    fn label(&self) -> &str;
    fn read_rpm(&self) -> Result<u32>;
    fn read_boost(&self) -> Result<u8>;
    fn set_boost(&self, value: u8) -> Result<()>;
    /// Valid boost range, in case a vendor uses one other than 0-255.
    fn boost_range(&self) -> (u8, u8);
    /// Can the current process write the boost? (/sys permissions)
    fn is_writable(&self) -> bool;
    /// Maximum RPM reported by the driver, if any (to show % of max).
    fn max_rpm(&self) -> Option<u32>;
}

/// A temperature sensor — also knows nothing about fans.
pub trait TempSensor: Send + Sync {
    /// "CPU", "GPU", etc.
    fn label(&self) -> &str;
    fn read_celsius(&self) -> Result<f32>;
}

/// The complete device — groups fans and sensors without knowing their detail.
pub trait ThermalDevice: Send + Sync {
    fn vendor_name(&self) -> &str;
    /// Name of the kernel driver backing this device.
    fn driver_name(&self) -> &str;
    /// Equipment model (DMI), if it can be read.
    fn model(&self) -> Option<String>;
    fn fans(&self) -> Vec<Box<dyn Fan>>;
    fn sensors(&self) -> Vec<Box<dyn TempSensor>>;
    fn supports_auto_curve(&self) -> bool;
}

/// Registered vendors, in trial order for automatic detection.
/// To add a vendor: implement the traits and add it here and in `create_device`.
pub const VENDORS: &[&str] = &["alienware"];

/// Automatic detection: tries each registered vendor and returns the first
/// one that finds compatible hardware.
pub fn detect_device() -> Result<Box<dyn ThermalDevice>> {
    for vendor in VENDORS {
        if let Ok(device) = create_device(vendor) {
            return Ok(device);
        }
    }
    Err(ThermalError::DeviceNotFound)
}

/// Explicit creation by vendor, for the manual fallback from Settings.
pub fn create_device(vendor: &str) -> Result<Box<dyn ThermalDevice>> {
    match vendor {
        "alienware" => Ok(Box::new(alienware::AlienwareDevice::detect()?)),
        other => Err(ThermalError::UnknownVendor(other.to_string())),
    }
}
