use std::fs;
use std::path::{Path, PathBuf};

use super::{thinkpad, Fan, Result, TempSensor, ThermalDevice, ThermalError};

const HWMON_ROOT: &str = "/sys/class/hwmon";
const DMI_ROOT: &str = "/sys/class/dmi/id";
pub const VENDORS: &[&str] = &[thinkpad::VENDOR, "lenovo", "hp", "asus"];

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
        let manufacturer = read_text(&dmi.join("sys_vendor"))
            .unwrap_or_default()
            .to_ascii_lowercase();
        let model = read_text(&dmi.join("product_name"));
        let family = read_text(&dmi.join("product_family"));
        let vendor = match manufacturer.as_str() {
            candidate
                if thinkpad::matches_device(
                    candidate,
                    model.as_deref(),
                    family.as_deref(),
                    || {
                        directories(hwmon).any(|path| {
                            read_text(&path.join("name")).as_deref() == Some(thinkpad::HWMON_NAME)
                        })
                    },
                ) =>
            {
                thinkpad::VENDOR
            }
            "lenovo" => "lenovo",
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
        let mut sources: Vec<_> = directories(hwmon)
            .filter_map(|path| fs::canonicalize(path).ok())
            .collect();
        sources.sort();
        sources.dedup();
        for path in sources {
            let Some(driver) = read_text(&path.join("name")) else {
                continue;
            };
            let vendor_source = match vendor {
                thinkpad::VENDOR => driver == thinkpad::HWMON_NAME,
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
    let device = path
        .parent()
        .filter(|parent| parent.ends_with("hwmon"))
        .and_then(Path::parent)
        .unwrap_or(path);
    device
        .strip_prefix("/sys/devices")
        .unwrap_or(device)
        .to_string_lossy()
        .into_owned()
}

fn read_text(path: &Path) -> Option<String> {
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
        let rpm = read_number(&self.input)?;
        if rpm == u16::MAX as u32 || rpm > 100_000 {
            return Err(ThermalError::InvalidValue(self.input.display().to_string()));
        }
        Ok(rpm)
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

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use crate::{commands, config::FanMode, db::Db, monitor, state::AppState};

    use super::*;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "thermal-berry-hwmon-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            fs::create_dir(&root).unwrap();
            Self(root)
        }

        fn write(&self, path: &str, value: &str) {
            let file = self.0.join(path);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, value).unwrap();
        }

        fn discover(&self, vendor: Option<&str>) -> Result<HwmonDevice> {
            HwmonDevice::discover(&self.0.join("dmi"), &self.0.join("class"), vendor)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn identifies_each_family_and_ignores_other_vendors_fans() {
        for (manufacturer, family, driver, expected_vendor) in [
            ("LENOVO", "ThinkPad T Series", "thinkpad", "thinkpad"),
            ("LENOVO", "Yoga", "yogafan", "lenovo"),
            ("Hewlett-Packard", "EliteBook", "hp_wmi_sensors", "hp"),
            ("ASUSTeK COMPUTER INC.", "ROG", "asus", "asus"),
        ] {
            let fixture = Fixture::new();
            fixture.write("dmi/sys_vendor", manufacturer);
            fixture.write("dmi/product_family", family);
            fixture.write("class/hwmon4/name", driver);
            fixture.write("class/hwmon4/fan3_input", "2100");
            fixture.write("class/hwmon9/name", "coretemp");
            fixture.write("class/hwmon9/temp2_input", "48000");
            fixture.write("class/hwmon9/temp5_input", "corrupt");
            fixture.write("class/hwmon9/fan1_input", "700");

            let device = fixture.discover(Some(expected_vendor)).unwrap();
            assert_eq!(device.vendor_name(), expected_vendor);
            assert_eq!(device.fans.len(), 1);
            assert_eq!(device.fans[0].read_rpm().unwrap(), 2100);
            assert_eq!(device.sensors.len(), 2);
            assert_eq!(device.sensors[0].read_celsius().unwrap(), 48.0);
            assert!(device.sensors[1].read_celsius().is_err());
            assert!(fixture.discover(Some("unmatched")).is_err());
        }
    }

    #[test]
    fn fan_identity_is_stable_when_hwmon_is_renumbered() {
        let fixture = Fixture::new();
        fixture.write("dmi/sys_vendor", "LENOVO");
        fixture.write("devices/thinkpad/hwmon/hwmon2/name", "thinkpad");
        fixture.write("devices/thinkpad/hwmon/hwmon2/fan1_input", "1500");
        let class = fixture.0.join("class");
        fs::create_dir(&class).unwrap();
        let chip = fixture.0.join("devices/thinkpad/hwmon");
        symlink(chip.join("hwmon2"), class.join("hwmon2")).unwrap();
        let first = fixture.discover(None).unwrap();
        assert_eq!(first.vendor_name(), "thinkpad");

        fs::rename(chip.join("hwmon2"), chip.join("hwmon7")).unwrap();
        fs::remove_file(class.join("hwmon2")).unwrap();
        symlink(chip.join("hwmon7"), class.join("hwmon7")).unwrap();

        let second = fixture.discover(None).unwrap();
        assert_eq!(first.fans[0].id(), second.fans[0].id());
        assert_eq!(second.fans[0].read_rpm().unwrap(), 1500);
    }

    #[test]
    fn read_only_devices_monitor_and_enter_maintenance_without_writing() {
        let fixture = Fixture::new();
        fixture.write("dmi/sys_vendor", "LENOVO");
        fixture.write("class/hwmon3/name", "thinkpad");
        fixture.write("class/hwmon3/fan1_input", "1900");
        fixture.write("class/hwmon3/pwm1", "90");
        let device = fixture.discover(None).unwrap();
        let fan_id = device.fans[0].id().to_owned();
        assert!(!device.fans[0].is_writable());

        let state = Arc::new(AppState::new(
            Some(Box::new(device)),
            Default::default(),
            Db::open_in_memory().unwrap(),
        ));
        commands::restore_control(&state);
        let sample = monitor::tick(&state, 2.0).unwrap();
        assert!(sample.errors.is_empty());
        assert_eq!(sample.fans[0].rpm, Some(1900));
        assert_eq!(sample.fans[0].boost_percent, None);
        assert!(state.lock().control.applied_mode.is_none());

        let epoch = state.lock().config.control_epoch;
        assert!(matches!(
            commands::apply_fan_boost(&state, fan_id, 60, epoch),
            Err(ThermalError::Unsupported)
        ));
        assert!(matches!(
            commands::apply_mode(&state, FanMode::Auto),
            Err(ThermalError::Unsupported)
        ));
        assert!(matches!(
            commands::apply_all_fans_boost(&state, true, 100),
            Err(ThermalError::Unsupported)
        ));
        commands::begin_maintenance(&state).unwrap();
        assert!(monitor::tick(&state, 2.0).is_none());
        crate::fan_control::return_to_firmware(&state);

        assert_eq!(
            fs::read_to_string(fixture.0.join("class/hwmon3/pwm1")).unwrap(),
            "90"
        );
    }
}
