use windows::Wdk::Graphics::Direct3D::{
    D3DKMTCloseAdapter, D3DKMTEnumAdapters2, D3DKMTQueryAdapterInfo, D3DKMT_ADAPTERINFO,
    D3DKMT_ADAPTER_PERFDATA, D3DKMT_CLOSEADAPTER, D3DKMT_ENUMADAPTERS2,
    D3DKMT_PHYSICAL_ADAPTER_COUNT, D3DKMT_QUERYADAPTERINFO, KMTQAITYPE_ADAPTERPERFDATA,
    KMTQAITYPE_PHYSICALADAPTERCOUNT, KMTQUERYADAPTERINFOTYPE,
};

use crate::metrics::Temperature;

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

    fn highest_temperature(&self) -> Option<u32> {
        let mut count = D3DKMT_PHYSICAL_ADAPTER_COUNT::default();
        let count = if unsafe { self.query(KMTQAITYPE_PHYSICALADAPTERCOUNT, &mut count) } {
            count.Count
        } else {
            1
        };

        (0..count)
            .filter_map(|index| {
                let mut data = D3DKMT_ADAPTER_PERFDATA {
                    PhysicalAdapterIndex: index,
                    ..Default::default()
                };
                let supported = unsafe { self.query(KMTQAITYPE_ADAPTERPERFDATA, &mut data) };
                (supported && (1..=1500).contains(&data.Temperature)).then_some(data.Temperature)
            })
            .max()
    }
}

pub fn gpu_temperature() -> Option<Temperature> {
    let decicelsius = enumerate()?
        .iter()
        .filter_map(Adapter::highest_temperature)
        .max()?;
    Some(Temperature {
        id: "gpu".into(),
        label: "GPU temperature".into(),
        celsius: decicelsius as f32 / 10.0,
    })
}
