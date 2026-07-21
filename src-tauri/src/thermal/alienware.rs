use std::fs;
use std::path::{Path, PathBuf};

use super::{Fan, Result, TempSensor, ThermalDevice, ThermalError};

const HWMON_ROOT: &str = "/sys/class/hwmon";
const HWMON_NAME: &str = "alienware_wmi";
const DMI_PRODUCT: &str = "/sys/class/dmi/id/product_name";
/// Upper bound on enumerated fans/sensors: no real board exposes anywhere
/// near this many, so it just keeps a misbehaving driver from turning the
/// index scan into an unbounded loop.
const MAX_ENTRIES: u32 = 64;

/// Implementation for Alienware via the kernel's `alienware-wmi-wmax` driver.
///
/// The hwmon number changes between reboots, so detection dynamically
/// searches for the directory whose `name` is `alienware_wmi`. Fans and
/// sensors are enumerated by index (`fanN_input`, `tempN_input`) instead of
/// assuming there are always exactly two.
pub struct AlienwareDevice {
    hwmon_path: PathBuf,
}

impl AlienwareDevice {
    pub fn detect() -> Result<Self> {
        let root = Path::new(HWMON_ROOT);
        let entries = fs::read_dir(root).map_err(|e| ThermalError::from_io(root, e))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if read_trimmed(&path.join("name")).as_deref() == Some(HWMON_NAME) {
                return Ok(Self { hwmon_path: path });
            }
        }
        Err(ThermalError::DeviceNotFound)
    }

    pub fn hwmon_path(&self) -> &Path {
        &self.hwmon_path
    }
}

impl ThermalDevice for AlienwareDevice {
    fn vendor_name(&self) -> &str {
        "alienware"
    }

    fn driver_name(&self) -> &str {
        HWMON_NAME
    }

    fn model(&self) -> Option<String> {
        read_trimmed(Path::new(DMI_PRODUCT))
    }

    fn fans(&self) -> Vec<Box<dyn Fan>> {
        let mut fans: Vec<Box<dyn Fan>> = Vec::new();
        for index in 1..MAX_ENTRIES {
            let input = self.hwmon_path.join(format!("fan{index}_input"));
            if !input.exists() {
                break;
            }
            let label = read_trimmed(&self.hwmon_path.join(format!("fan{index}_label")))
                .unwrap_or_else(|| format!("Fan {index}"));
            fans.push(Box::new(AlienwareFan {
                hwmon_path: self.hwmon_path.clone(),
                id: format!("fan{index}"),
                index,
                label,
            }));
        }
        fans
    }

    fn sensors(&self) -> Vec<Box<dyn TempSensor>> {
        let mut sensors: Vec<Box<dyn TempSensor>> = Vec::new();
        for index in 1..MAX_ENTRIES {
            let input = self.hwmon_path.join(format!("temp{index}_input"));
            if !input.exists() {
                break;
            }
            let label = read_trimmed(&self.hwmon_path.join(format!("temp{index}_label")))
                .unwrap_or_else(|| format!("Sensor {index}"));
            sensors.push(Box::new(HwmonTempSensor {
                input_path: input,
                label,
            }));
        }
        sensors
    }

    fn supports_auto_curve(&self) -> bool {
        true
    }
}

struct AlienwareFan {
    hwmon_path: PathBuf,
    id: String,
    index: u32,
    label: String,
}

impl AlienwareFan {
    fn attr(&self, suffix: &str) -> PathBuf {
        self.hwmon_path.join(format!("fan{}_{suffix}", self.index))
    }
}

impl Fan for AlienwareFan {
    fn id(&self) -> &str {
        &self.id
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn read_rpm(&self) -> Result<u32> {
        read_number(&self.attr("input")).map(|v| v as u32)
    }

    fn read_boost(&self) -> Result<u8> {
        read_number(&self.attr("boost")).map(|v| v.clamp(0, 255) as u8)
    }

    fn set_boost(&self, value: u8) -> Result<()> {
        let path = self.attr("boost");
        fs::write(&path, value.to_string()).map_err(|e| ThermalError::from_io(&path, e))
    }

    fn boost_range(&self) -> (u8, u8) {
        (0, 255)
    }

    fn is_writable(&self) -> bool {
        // Opening for write doesn't modify the attribute; fails with EACCES if
        // the udev rule hasn't been installed yet.
        fs::OpenOptions::new()
            .write(true)
            .open(self.attr("boost"))
            .is_ok()
    }

    fn max_rpm(&self) -> Option<u32> {
        read_number(&self.attr("max")).ok().map(|v| v as u32)
    }
}

struct HwmonTempSensor {
    input_path: PathBuf,
    label: String,
}

impl TempSensor for HwmonTempSensor {
    fn label(&self) -> &str {
        &self.label
    }

    fn read_celsius(&self) -> Result<f32> {
        // hwmon reports temperatures in millidegrees C.
        read_number(&self.input_path).map(|v| v as f32 / 1000.0)
    }
}

fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn read_number(path: &Path) -> Result<i64> {
    let raw = fs::read_to_string(path).map_err(|e| ThermalError::from_io(path, e))?;
    raw.trim()
        .parse()
        .map_err(|_| ThermalError::InvalidValue(format!("{}: \"{}\"", path.display(), raw.trim())))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Requires real Alienware hardware; run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn detects_and_reads_real_hardware() {
        let device = AlienwareDevice::detect().expect("alienware_wmi hwmon not found");
        let fans = device.fans();
        let sensors = device.sensors();
        assert!(!fans.is_empty());
        assert!(!sensors.is_empty());
        for fan in &fans {
            let rpm = fan.read_rpm().unwrap();
            let boost = fan.read_boost().unwrap();
            println!("{} ({}): {rpm} RPM, boost {boost}", fan.id(), fan.label());
        }
        for sensor in &sensors {
            let temp = sensor.read_celsius().unwrap();
            println!("{}: {temp:.1}°C", sensor.label());
            assert!(temp > 0.0 && temp < 120.0);
        }
    }
}
