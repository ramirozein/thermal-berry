use std::sync::Arc;

use tauri::{AppHandle, Emitter};
use thermal_berry_core::state::{AppState, Sample};

pub use thermal_berry_core::monitor::HistoryWriter;
pub const TELEMETRY_EVENT: &str = "telemetry";

pub fn spawn(app: AppHandle, state: Arc<AppState>) -> HistoryWriter {
    let mut last_status = String::new();
    thermal_berry_core::monitor::spawn(state, move |sample, interval, device_available| {
        crate::metrics::publish_linux_temperatures(&app, sample, interval, device_available);
        if let Err(e) = app.emit_to("main", TELEMETRY_EVENT, sample) {
            eprintln!("thermal-berry: error emitting telemetry: {e}");
        }
        let status = status_line(sample);
        if status != last_status {
            crate::tray::update_status(&app, &status);
            last_status = status;
        }
    })
}

fn status_line(sample: &Sample) -> String {
    let mut parts: Vec<String> = sample
        .temps
        .iter()
        .map(|t| format!("{} {:.0}°C", t.label, t.celsius))
        .collect();
    if !sample.errors.is_empty()
        || sample
            .control
            .as_ref()
            .is_some_and(|c| c.error.is_some() || c.suspended)
    {
        parts.insert(0, "Control / telemetry error".into());
    }
    parts.extend(
        sample
            .fans
            .iter()
            .filter_map(|f| f.rpm.map(|rpm| format!("{rpm} RPM"))),
    );
    if parts.is_empty() {
        "No readings".to_string()
    } else {
        parts.join(" · ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_shows_control_errors_in_the_status_line() {
        let sample = Sample {
            timestamp_ms: 0,
            temps: vec![],
            fans: vec![],
            control: None,
            errors: vec!["could not write fan1".into()],
        };
        assert!(status_line(&sample).contains("error"));
    }
}
