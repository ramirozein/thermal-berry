use windows::Wdk::Graphics::Direct3D::{
    D3DKMTCloseAdapter, D3DKMTEnumAdapters2, D3DKMTQueryAdapterInfo, D3DKMT_ADAPTERINFO,
    D3DKMT_ADAPTER_PERFDATA, D3DKMT_ADAPTER_PERFDATACAPS, D3DKMT_CLOSEADAPTER,
    D3DKMT_ENUMADAPTERS2, D3DKMT_PHYSICAL_ADAPTER_COUNT, D3DKMT_QUERYADAPTERINFO,
    KMTQAITYPE_ADAPTERPERFDATA, KMTQAITYPE_ADAPTERPERFDATA_CAPS, KMTQAITYPE_PHYSICALADAPTERCOUNT,
    KMTQUERYADAPTERINFOTYPE,
};

use crate::metrics::{FanSpeed, Temperature};

struct Adapter(u32);

impl Drop for Adapter {
    fn drop(&mut self) {
        let _ = unsafe { D3DKMTCloseAdapter(&D3DKMT_CLOSEADAPTER { hAdapter: self.0 }) };
    }
}

fn enumerate() -> Option<Vec<Adapter>> {
    let mut request = D3DKMT_ENUMADAPTERS2::default();
    if unsafe { D3DKMTEnumAdapters2(&mut request) }.0 != 0 || request.NumAdapters == 0 {
        return None;
    }

    let mut entries = vec![D3DKMT_ADAPTERINFO::default(); request.NumAdapters as usize];
    request.pAdapters = entries.as_mut_ptr();
    if unsafe { D3DKMTEnumAdapters2(&mut request) }.0 != 0 {
        return None;
    }
    entries.truncate(request.NumAdapters as usize);
    Some(
        entries
            .into_iter()
            .map(|entry| Adapter(entry.hAdapter))
            .collect(),
    )
}

impl Adapter {
    unsafe fn query<T>(&self, kind: KMTQUERYADAPTERINFOTYPE, output: &mut T) -> bool {
        let mut request = D3DKMT_QUERYADAPTERINFO {
            hAdapter: self.0,
            Type: kind,
            pPrivateDriverData: std::ptr::from_mut(output).cast(),
            PrivateDriverDataSize: std::mem::size_of::<T>() as u32,
        };
        unsafe { D3DKMTQueryAdapterInfo(&mut request) }.0 == 0
    }

    fn physical_performance(&self) -> impl Iterator<Item = D3DKMT_ADAPTER_PERFDATA> + '_ {
        let mut count = D3DKMT_PHYSICAL_ADAPTER_COUNT::default();
        let count = if unsafe { self.query(KMTQAITYPE_PHYSICALADAPTERCOUNT, &mut count) } {
            count.Count
        } else {
            1
        };

        (0..count).filter_map(|index| {
            let mut data = D3DKMT_ADAPTER_PERFDATA {
                PhysicalAdapterIndex: index,
                ..Default::default()
            };
            unsafe { self.query(KMTQAITYPE_ADAPTERPERFDATA, &mut data) }.then_some(data)
        })
    }

    fn fan_rpm(&self, data: &D3DKMT_ADAPTER_PERFDATA) -> Option<u32> {
        match data.FanRPM {
            1..=100_000 => Some(data.FanRPM),
            0 => {
                // Some GPUs leave the RPM field at zero because they have no fan.
                let mut caps = D3DKMT_ADAPTER_PERFDATACAPS {
                    PhysicalAdapterIndex: data.PhysicalAdapterIndex,
                    ..Default::default()
                };
                let supported = unsafe { self.query(KMTQAITYPE_ADAPTERPERFDATA_CAPS, &mut caps) };
                (supported && (1..=100_000).contains(&caps.MaxFanRPM)).then_some(0)
            }
            _ => None,
        }
    }
}

pub(crate) fn gpu_readings() -> (Option<Temperature>, Vec<FanSpeed>) {
    let mut highest_temperature = None;
    let mut fans = Vec::new();
    for (adapter_index, adapter) in enumerate().unwrap_or_default().iter().enumerate() {
        for reading in adapter.physical_performance() {
            if (1..=1500).contains(&reading.Temperature) {
                highest_temperature = highest_temperature.max(Some(reading.Temperature));
            }
            if let Some(rpm) = adapter.fan_rpm(&reading) {
                fans.push(FanSpeed {
                    id: format!("gpu-fan-{adapter_index}-{}", reading.PhysicalAdapterIndex),
                    label: format!("GPU fan {}", fans.len() + 1),
                    rpm,
                });
            }
        }
    }
    let temperature = highest_temperature.map(|value| Temperature {
        id: "gpu".into(),
        label: "GPU temperature".into(),
        celsius: value as f32 / 10.0,
    });
    (temperature, fans)
}
