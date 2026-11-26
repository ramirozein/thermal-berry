use std::{
    fs,
    path::{Path, PathBuf},
};

use super::GpuLoad;

pub(crate) struct Amd {
    device: PathBuf,
    hwmon: Option<PathBuf>,
}

pub(super) fn discover(root: &Path) -> Vec<Amd> {
    let mut devices = Vec::new();
    for entry in fs::read_dir(root).into_iter().flatten().flatten() {
        let name = entry.file_name();
        if !name
            .to_str()
            .and_then(|s| s.strip_prefix("card"))
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        {
            continue;
        }
        let device = entry.path().join("device");
        if fs::read_to_string(device.join("vendor"))
            .ok()
            .as_deref()
            .map(str::trim)
            != Some("0x1002")
        {
            continue;
        }
        let hwmon = fs::read_dir(device.join("hwmon"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .find(|p| fs::read_to_string(p.join("name")).is_ok_and(|s| s.trim() == "amdgpu"));
        devices.push(Amd { device, hwmon });
    }
    devices
}

fn number(path: PathBuf) -> Option<u32> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

impl Amd {
    pub fn read(&self) -> GpuLoad {
        let percent = |name| {
            number(self.device.join(name))
                .filter(|n| *n <= 100)
                .map(f64::from)
        };
        let hwmon = |name| self.hwmon.as_ref().and_then(|p| number(p.join(name)));
        GpuLoad {
            percent: percent("gpu_busy_percent"),
            memory_percent: percent("mem_busy_percent"),
            celsius: hwmon("temp1_input")
                .filter(|t| *t <= 150_000)
                .map(|t| f64::from(t) / 1000.0),
            power_w: hwmon("power1_average").map(|p| f64::from(p) / 1_000_000.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maps_only_amd_cards_and_converts_kernel_units_without_inventing_missing_load() {
        use std::os::unix::fs::DirBuilderExt;
        let root =
            std::env::temp_dir().join(format!("thermal-berry-amdgpu-{}", std::process::id()));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let gpu = root.join("card17/device");
        let hw = gpu.join("hwmon/hwmon42");
        fs::create_dir_all(&hw).unwrap();
        for name in ["card17-DP-1", "card5"] {
            let device = root.join(name).join("device");
            fs::create_dir_all(&device).unwrap();
            fs::write(
                device.join("vendor"),
                if name == "card5" { "0x8086" } else { "0x1002" },
            )
            .unwrap();
        }
        fs::write(gpu.join("vendor"), "0x1002\n").unwrap();
        fs::write(hw.join("name"), "amdgpu\n").unwrap();
        fs::write(hw.join("temp1_input"), "52500\n").unwrap();
        fs::write(hw.join("power1_average"), "38500000").unwrap();
        fs::write(gpu.join("gpu_busy_percent"), "0").unwrap();
        fs::write(gpu.join("mem_busy_percent"), "23").unwrap();
        let devices = discover(&root);
        assert_eq!(devices.len(), 1);
        let reading = devices[0].read();
        assert_eq!(
            (
                reading.celsius,
                reading.power_w,
                reading.percent,
                reading.memory_percent
            ),
            (Some(52.5), Some(38.5), Some(0.0), Some(23.0))
        );
        fs::write(gpu.join("gpu_busy_percent"), "101").unwrap();
        assert_eq!(devices[0].read().percent, None);
        fs::remove_file(gpu.join("gpu_busy_percent")).unwrap();
        assert_eq!(devices[0].read().percent, None);
        fs::remove_dir_all(root).unwrap();
    }
}
