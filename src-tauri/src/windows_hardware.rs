use std::time::{Duration, Instant};

use windows::core::{w, BSTR, PCWSTR};
use windows::Win32::System::{
    Com::{
        CoCreateInstance, CoInitializeEx, CoSetProxyBlanket, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED, EOAC_NONE, RPC_C_AUTHN_LEVEL_PKT_PRIVACY,
        RPC_C_IMP_LEVEL_IMPERSONATE,
    },
    Rpc::{RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE},
    Variant::{VariantClear, VARIANT, VT_BSTR, VT_I4, VT_UI4},
    Wmi::{
        IWbemClassObject, IWbemLocator, IWbemServices, WbemLocator, WBEM_FLAG_CONNECT_USE_MAX_WAIT,
        WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY,
    },
};

use crate::metrics::{FanSpeed, HardwareModel, Temperature};

const QUERY_TIMEOUT: Duration = Duration::from_millis(400);
const MAX_READINGS: usize = 32;

pub(crate) struct Wmi {
    system: Option<IWbemServices>,
    acpi: Option<IWbemServices>,
    _apartment: ComApartment,
}

struct ComApartment;

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() }
    }
}

impl Wmi {
    pub(crate) fn local() -> Option<Self> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .ok()?;
        let apartment = ComApartment;
        let locator: IWbemLocator =
            unsafe { CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER) }.ok()?;
        Some(Self {
            system: connect(&locator, r"ROOT\CIMV2"),
            acpi: connect(&locator, r"ROOT\WMI"),
            _apartment: apartment,
        })
    }

    pub(crate) fn hardware_model(&self) -> Option<HardwareModel> {
        const QUERY: &str = "SELECT Manufacturer, Model, SystemFamily FROM Win32_ComputerSystem";
        let row = query(self.system.as_ref()?, QUERY, 1).ok()?.pop()?;
        let manufacturer = property(&row, w!("Manufacturer"))?.text()?;
        let model = property(&row, w!("Model"))?.text()?;
        let family = property(&row, w!("SystemFamily")).and_then(|value| value.text());
        let thinkpad = family
            .iter()
            .chain(std::iter::once(&model))
            .any(|value| value.to_ascii_lowercase().contains("thinkpad"));
        let vendor = match manufacturer.to_ascii_lowercase().as_str() {
            "lenovo" if thinkpad => "thinkpad",
            "lenovo" => "lenovo",
            "hp" | "hewlett-packard" => "hp",
            "asus" | "asustek computer inc." => "asus",
            _ => return None,
        };
        Some(HardwareModel { vendor, model })
    }

    pub(crate) fn temperatures(&self) -> windows::core::Result<Vec<Temperature>> {
        const QUERY: &str =
            "SELECT CurrentTemperature FROM MSAcpi_ThermalZoneTemperature WHERE Active = TRUE";
        let Some(acpi) = self.acpi.as_ref() else {
            return Ok(Vec::new());
        };
        Ok(query(acpi, QUERY, MAX_READINGS)?
            .iter()
            .enumerate()
            .filter_map(|(index, row)| {
                let raw = property(row, w!("CurrentTemperature"))?.unsigned()?;
                let celsius = raw as f32 / 10.0 - 273.15;
                (0.0 < celsius && celsius <= 150.0).then(|| Temperature {
                    id: format!("acpi-zone-{index}"),
                    label: format!("ACPI thermal zone {}", index + 1),
                    celsius,
                })
            })
            .collect())
    }

    pub(crate) fn fan_speeds(&self) -> windows::core::Result<Vec<FanSpeed>> {
        const QUERY: &str = "SELECT Name, CurrentReading FROM CIM_Tachometer";
        let Some(system) = self.system.as_ref() else {
            return Ok(Vec::new());
        };
        Ok(query(system, QUERY, MAX_READINGS)?
            .iter()
            .enumerate()
            .filter_map(|(index, row)| {
                let rpm = property(row, w!("CurrentReading"))?.unsigned()?;
                if rpm > 100_000 {
                    return None;
                }
                let label = property(row, w!("Name"))
                    .and_then(|value| value.text())
                    .unwrap_or_else(|| format!("Fan {}", index + 1));
                Some(FanSpeed {
                    id: format!("tachometer-{index}"),
                    label,
                    rpm,
                })
            })
            .collect())
    }
}

fn connect(locator: &IWbemLocator, namespace: &str) -> Option<IWbemServices> {
    let empty = BSTR::new();
    unsafe {
        let service = locator
            .ConnectServer(
                &BSTR::from(namespace),
                &empty,
                &empty,
                &empty,
                WBEM_FLAG_CONNECT_USE_MAX_WAIT.0,
                &empty,
                None,
            )
            .ok()?;
        CoSetProxyBlanket(
            &service,
            RPC_C_AUTHN_WINNT,
            RPC_C_AUTHZ_NONE,
            PCWSTR::null(),
            RPC_C_AUTHN_LEVEL_PKT_PRIVACY,
            RPC_C_IMP_LEVEL_IMPERSONATE,
            None,
            EOAC_NONE,
        )
        .ok()?;
        Some(service)
    }
}

fn query(
    service: &IWbemServices,
    statement: &str,
    limit: usize,
) -> windows::core::Result<Vec<IWbemClassObject>> {
    let cursor = unsafe {
        service.ExecQuery(
            &BSTR::from("WQL"),
            &BSTR::from(statement),
            WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
            None,
        )?
    };
    let deadline = Instant::now() + QUERY_TIMEOUT;
    let mut rows = Vec::new();
    while rows.len() < limit {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let mut row = [None];
        let mut returned = 0;
        let status =
            unsafe { cursor.Next(remaining.as_millis().max(1) as i32, &mut row, &mut returned) };
        status.ok()?;
        if returned == 0 {
            break;
        }
        let Some(row) = row[0].take() else { break };
        rows.push(row);
    }
    Ok(rows)
}

#[derive(Default)]
struct Property(VARIANT);

impl Drop for Property {
    fn drop(&mut self) {
        let _ = unsafe { VariantClear(&mut self.0) };
    }
}

impl Property {
    fn unsigned(&self) -> Option<u32> {
        unsafe {
            let value = &*self.0.Anonymous.Anonymous;
            match value.vt {
                VT_I4 => u32::try_from(value.Anonymous.lVal).ok(),
                VT_UI4 => Some(value.Anonymous.ulVal),
                _ => None,
            }
        }
    }

    fn text(&self) -> Option<String> {
        let value = unsafe {
            let value = &*self.0.Anonymous.Anonymous;
            if value.vt != VT_BSTR {
                return None;
            }
            &*value.Anonymous.bstrVal
        };
        if value.len() > 256 {
            return None;
        }
        let text = value.to_string();
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_owned())
    }
}

fn property(row: &IWbemClassObject, name: PCWSTR) -> Option<Property> {
    let mut value = Property::default();
    unsafe { row.Get(name, 0, &mut value.0, None, None) }.ok()?;
    Some(value)
}
