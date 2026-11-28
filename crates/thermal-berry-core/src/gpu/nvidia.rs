//! The small read-only subset of the NVML C ABI used for identification.
//! All calls run on the same sampling thread. No driver is linked at build time.
use libloading::Library;
use std::ffi::{c_int, c_uint, c_void};
use std::ptr;

use super::GpuLoad;

type Device = *mut c_void;
type Init = unsafe extern "C" fn() -> c_int;
type Count = unsafe extern "C" fn(*mut c_uint) -> c_int;
type ByIndex = unsafe extern "C" fn(c_uint, *mut Device) -> c_int;
type Temperature = unsafe extern "C" fn(Device, c_uint, *mut c_uint) -> c_int;
type Utilization = unsafe extern "C" fn(Device, *mut Rates) -> c_int;
type Power = unsafe extern "C" fn(Device, *mut c_uint) -> c_int;

#[repr(C)]
#[derive(Default)]
struct Rates {
    gpu: c_uint,
    memory: c_uint,
}

pub(crate) struct Nvml {
    library: Library,
    shutdown: Init,
}

impl Nvml {
    pub fn open() -> Option<Self> {
        // SAFETY: use the system driver's ABI and keep the library loaded until shutdown.
        unsafe {
            // Load only from system driver directories, including common container mounts.
            let library = [
                "/usr/lib/x86_64-linux-gnu",
                "/usr/lib64",
                "/usr/lib",
                "/run/opengl-driver/lib",
                "/usr/local/nvidia/lib64",
                "/usr/lib/wsl/lib",
            ]
            .iter()
            .find_map(|dir| {
                Library::new(std::path::Path::new(dir).join("libnvidia-ml.so.1")).ok()
            })?;
            let shutdown = *library.get::<Init>(b"nvmlShutdown\0").ok()?;
            (library.get::<Init>(b"nvmlInit_v2\0").ok()?() == 0).then_some(())?;
            Some(Self { library, shutdown })
        }
    }

    pub fn count(&self) -> Option<u32> {
        let mut count = 0;
        // SAFETY: NVML writes one unsigned integer to this live output pointer.
        unsafe {
            (self.library.get::<Count>(b"nvmlDeviceGetCount_v2\0").ok()?(&mut count) == 0)
                .then_some(count)
        }
    }

    pub fn read(&self) -> GpuLoad {
        let mut reading = GpuLoad::default();
        let mut device = ptr::null_mut();
        // SAFETY: signatures match the corresponding NVML functions. The handle is
        // obtained from this initialized library and used only on successful lookup.
        unsafe {
            let Ok(get) = self
                .library
                .get::<ByIndex>(b"nvmlDeviceGetHandleByIndex_v2\0")
            else {
                return reading;
            };
            if get(0, &mut device) != 0 || device.is_null() {
                return reading;
            }
            if let Ok(get) = self
                .library
                .get::<Temperature>(b"nvmlDeviceGetTemperature\0")
            {
                let mut t = 0;
                if get(device, 0, &mut t) == 0 && t <= 150 {
                    reading.celsius = Some(f64::from(t));
                }
            }
            if let Ok(get) = self
                .library
                .get::<Utilization>(b"nvmlDeviceGetUtilizationRates\0")
            {
                let mut rates = Rates::default();
                if get(device, &mut rates) == 0 {
                    reading.percent = (rates.gpu <= 100).then_some(f64::from(rates.gpu));
                    reading.memory_percent =
                        (rates.memory <= 100).then_some(f64::from(rates.memory));
                }
            }
            if let Ok(get) = self.library.get::<Power>(b"nvmlDeviceGetPowerUsage\0") {
                let mut mw = 0;
                if get(device, &mut mw) == 0 {
                    reading.power_w = Some(f64::from(mw) / 1000.0);
                }
            }
        }
        reading
    }
}

impl Drop for Nvml {
    fn drop(&mut self) {
        unsafe {
            (self.shutdown)();
        }
    }
}
