use std::fs;
use std::path::{Path, PathBuf};

use super::{Fan, Result, TempSensor, ThermalDevice, ThermalError};

const HWMON_ROOT: &str = "/sys/class/hwmon";
const DMI_ROOT: &str = "/sys/class/dmi/id";
pub const VENDORS: &[&str] = &["thinkpad", "lenovo", "hp", "asus"];

pub struct HwmonDevice {
    vendor: &'static str,
    model: Option<String>,
    fans: Vec<HwmonFan>,
    sensors: Vec<HwmonTemperature>,
}

impl HwmonDevice {
    pub fn detect(requested: Option<&str>) -> Result<Self> {
        Self::discover(Path::new(DMI_ROOT), Path::new(HWMON_ROOT), requested)
    }

    fn discover(dmi: &Path, hwmon: &Path, requested: Option<&str>) -> Result<Self> {
        let manufacturer = read_text(&dmi.join("sys_vendor")).unwrap_or_default();
        let model = read_text(&dmi.join("product_name"));
        let family = read_text(&dmi.join("product_family")).unwrap_or_default();
        let vendor = match manufacturer.to_ascii_lowercase().as_str() {
            "lenovo"
                if family.to_ascii_lowercase().contains("thinkpad")
                    || model
                        .as_deref()
                        .is_some_and(|name| name.to_ascii_lowercase().contains("thinkpad"))
                    || directories(hwmon).any(|path| {
                        read_text(&path.join("name")).as_deref() == Some("thinkpad")
                    }) =>
            {
                "thinkpad"
            }
            "lenovo" => "lenovo",
            "ibm"
                if family.to_ascii_lowercase().contains("thinkpad")
                    || model
                        .as_deref()
                        .is_some_and(|name| name.to_ascii_lowercase().contains("thinkpad")) =>
            {
                "thinkpad"
            }
            "hp" | "hewlett-packard" => "hp",
            "asus" | "asustek computer inc." => "asus",
            _ => return Err(ThermalError::DeviceNotFound),
        };
        if requested.is_some_and(|requested| requested != vendor) {
            return Err(ThermalError::DeviceNotFound);
        }

        let mut device = Self {
            vendor,
            model,
            fans: Vec::new(),
            sensors: Vec::new(),
        };
        let mut sources: Vec<_> = directories(hwmon).collect();
        sources.sort_by_cached_key(|path| identity(path));
        for path in sources {
            let Some(driver) = read_text(&path.join("name")) else {
                continue;
            };
            let vendor_source = match vendor {
                "thinkpad" => driver == "thinkpad",
                "lenovo" => matches!(driver.as_str(), "yogafan" | "lenovo_shm_sensors"),
                "hp" => matches!(driver.as_str(), "hp" | "hp_wmi_sensors"),
                "asus" => matches!(
                    driver.as_str(),
                    "asus" | "asus_wmi_sensors" | "asus_ec_sensors"
                ),
                _ => false,
            };
            let standard_temperature = matches!(
                driver.as_str(),
                "coretemp" | "k10temp" | "zenpower" | "amdgpu" | "nouveau" | "cpu_thermal"
            );
            if !vendor_source && !standard_temperature {
                continue;
            }

            let source = identity(&path);
            for index in 1..=64 {
                let fan = format!("fan{index}");
                let input = path.join(format!("{fan}_input"));
                if vendor_source && input.is_file() {
                    let id = format!("{driver}:{source}:{fan}");
                    if !device.fans.iter().any(|existing| existing.id == id) {
                        device.fans.push(HwmonFan {
                            id,
                            label: read_text(&path.join(format!("{fan}_label")))
                                .unwrap_or_else(|| format!("{driver} fan {index}")),
                            input,
                        });
                    }
                }

                let temp = format!("temp{index}");
                let input = path.join(format!("{temp}_input"));
                if input.is_file() {
                    let label = read_text(&path.join(format!("{temp}_label")))
                        .unwrap_or_else(|| format!("Sensor {index}"));
                    device.sensors.push(HwmonTemperature {
                        label: format!("{driver} · {label}"),
                        input,
                    });
                }
            }
        }
        Ok(device)
    }
}

fn directories(root: &Path) -> impl Iterator<Item = PathBuf> {
    fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .and_then(|name| name.strip_prefix("hwmon"))
                .is_some_and(|number| {
                    !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
                })
        })
        .map(|entry| entry.path())
}

fn identity(path: &Path) -> String {
    let resolved = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let device = resolved
        .parent()
        .filter(|parent| parent.ends_with("hwmon"))
        .and_then(Path::parent)
        .unwrap_or(&resolved);
    device
        .strip_prefix("/sys/devices")
        .unwrap_or(device)
        .to_string_lossy()
        .into_owned()
}

pub(super) fn read_text(path: &Path) -> Option<String> {
    let value = fs::read_to_string(path).ok()?.trim().to_owned();
    (!value.is_empty() && value.len() <= 256).then_some(value)
}

fn read_number<T: std::str::FromStr>(path: &Path) -> Result<T> {
    fs::read_to_string(path)
        .map_err(|error| ThermalError::from_io(path, error))?
        .trim()
        .parse()
        .map_err(|_| ThermalError::InvalidValue(path.display().to_string()))
}

impl ThermalDevice for HwmonDevice {
    fn vendor_name(&self) -> &str {
        self.vendor
    }
    fn driver_name(&self) -> &str {
        "hwmon"
    }
    fn model(&self) -> Option<String> {
        self.model.clone()
    }
    fn fans(&self) -> Vec<Box<dyn Fan>> {
        self.fans
            .iter()
            .cloned()
            .map(|fan| Box::new(fan) as Box<dyn Fan>)
            .collect()
    }
    fn sensors(&self) -> Vec<Box<dyn TempSensor>> {
        self.sensors
            .iter()
            .cloned()
            .map(|sensor| Box::new(sensor) as Box<dyn TempSensor>)
            .collect()
    }
    fn supports_auto_curve(&self) -> bool {
        false
    }
    fn supports_fan_control(&self) -> bool {
        false
    }
}

#[derive(Clone)]
struct HwmonFan {
    id: String,
    label: String,
    input: PathBuf,
}

impl Fan for HwmonFan {
    fn id(&self) -> &str {
        &self.id
    }
    fn label(&self) -> &str {
        &self.label
    }
    fn read_rpm(&self) -> Result<u32> {
        read_number(&self.input)
    }
    fn read_boost(&self) -> Result<u8> {
        Err(ThermalError::Unsupported)
    }
    fn set_boost(&self, _value: u8) -> Result<()> {
        Err(ThermalError::Unsupported)
    }
    fn boost_range(&self) -> (u8, u8) {
        (0, 0)
    }
    fn supports_boost(&self) -> bool {
        false
    }
    fn is_writable(&self) -> bool {
        false
    }
    fn max_rpm(&self) -> Option<u32> {
        None
    }
}

#[derive(Clone)]
struct HwmonTemperature {
    label: String,
    input: PathBuf,
}

impl TempSensor for HwmonTemperature {
    fn label(&self) -> &str {
        &self.label
    }
    fn read_celsius(&self) -> Result<f32> {
        read_number::<i32>(&self.input).map(|value| value as f32 / 1000.0)
    }
}
