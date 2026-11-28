mod amd;
#[cfg(feature = "nvidia")]
mod nvidia;

use std::path::Path;

#[derive(Default, Clone, Copy)]
pub(crate) struct GpuLoad {
    pub celsius: Option<f64>,
    pub percent: Option<f64>,
    pub memory_percent: Option<f64>,
    pub power_w: Option<f64>,
}

pub(crate) enum Gpu {
    Amd(amd::Amd),
    #[cfg(feature = "nvidia")]
    Nvidia(nvidia::Nvml),
}

impl Gpu {
    pub fn discover() -> Option<Self> {
        let amd = amd::discover(Path::new("/sys/class/drm"));
        #[cfg(feature = "nvidia")]
        let nvidia = nvidia::Nvml::open();
        #[cfg(feature = "nvidia")]
        if let Some(nvidia) = nvidia {
            match nvidia.count() {
                Some(0) => {}
                Some(1) if amd.is_empty() => return Some(Self::Nvidia(nvidia)),
                _ => return None,
            }
        }
        (amd.len() == 1).then(|| Self::Amd(amd.into_iter().next().unwrap()))
    }

    pub fn read(&self) -> GpuLoad {
        match self {
            Self::Amd(gpu) => gpu.read(),
            #[cfg(feature = "nvidia")]
            Self::Nvidia(gpu) => gpu.read(),
        }
    }
}
