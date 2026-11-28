use super::Result;
use std::{
    io::{Read, Write},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use thermal_berry_core::{calibration, db::Db, thermal};

static STOP: AtomicBool = AtomicBool::new(false);
extern "C" fn stop(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

struct Inhibitor(Child);
impl Inhibitor {
    fn acquire() -> Result<Self> {
        // cat echoes only after logind has granted the inhibitor to systemd-inhibit.
        let mut child = Self(
            Command::new("/usr/bin/systemd-inhibit")
                .args([
                    "--what=sleep",
                    "--mode=block",
                    "--who=Thermal Berry",
                    "--why=Local fan calibration",
                    "/usr/bin/cat",
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()?,
        );
        child.0.stdin.as_mut().unwrap().write_all(b"ok\n")?;
        let mut response = [0; 3];
        child.0.stdout.as_mut().unwrap().read_exact(&mut response)?;
        if &response != b"ok\n" {
            return Err("sleep inhibitor unavailable".into());
        }
        Ok(child)
    }
}

impl Drop for Inhibitor {
    fn drop(&mut self) {
        drop(self.0.stdin.take());
        let _ = self.0.wait();
    }
}

pub fn calibrate(cpu_only: bool, check_only: bool) -> Result<()> {
    let db = Db::open()?;
    let config = db.load_config();
    let device = thermal::configured_device(config.vendor_override.as_deref()).map(Arc::from);
    if check_only {
        let (sample, observation) = calibration::probe(device.as_ref());
        let joint =
            observation.gpu_percent.is_some() && observation.gpu_temp_c.is_some() && !cpu_only;
        let channels = device
            .as_ref()
            .ok_or("no supported thermal device".to_string())
            .and_then(|device| {
                calibration::controllable_fans(device.as_ref()).map_err(|e| e.to_string())
            });
        let reading = channels
            .as_ref()
            .ok()
            .map(|fans| calibration::preflight(fans, &sample, &observation, !joint))
            .transpose();
        let ready = matches!(reading, Ok(Some(_)));
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "scope": if joint {"cpu+gpu"} else {"cpu-only"},
                "ready": ready,
                "fan_topology": match sample.fans.len() { 1 => "single", 2 => "dual", _ => "unsupported" },
                "spin_up_required": reading.as_ref().ok().and_then(Option::as_ref),
                "fan_preflight_error": channels.err(),
                "reading_error": reading.err().map(|e| e.to_string()),
                "sample": sample,
                "observation": observation
            }))?
        );
        return if ready {
            Ok(())
        } else {
            Err("calibration preflight failed".into())
        };
    }
    let device = device.ok_or("no supported thermal device")?;
    calibration::controllable_fans(device.as_ref())?;
    let retention_ms = u64::from(config.retention_days()) * 24 * 3600 * 1000;
    db.purge_older_than(thermal_berry_core::readings::now_ms().saturating_sub(retention_ms))?;
    // SAFETY: the handler only sets an atomic flag. Cleanup runs on the normal thread.
    for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        if unsafe { libc::signal(signal, stop as *const () as libc::sighandler_t) } == libc::SIG_ERR
        {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    let mut inhibitor = Inhibitor::acquire()?;
    eprintln!("45-minute manual calibration. Apply the indicated local compute loads. Temporary fan control returns to firmware at exit.");
    let session = calibration::record(
        device,
        &db,
        cpu_only,
        || !STOP.load(Ordering::Relaxed) && matches!(inhibitor.0.try_wait(), Ok(None)),
        |stage| match stage {
            Some(stage) => {
                let second = stage
                    .second_fan_percent
                    .map(|p| format!(", F2 {p}%"))
                    .unwrap_or_default();
                eprintln!(
                    "CPU {}%, GPU {}%, F1 {}%{second}",
                    stage.cpu_percent, stage.gpu_percent, stage.fan_percent
                );
            }
            None => eprintln!("Stop calibration workloads"),
        },
    )?;
    let rows = db.observations(session.start_ms, session.end_ms)?;
    thermal_berry_core::identify::validate_session(&rows, session.cpu_only)?;
    println!(
        "Recorded {} ticks. Identify with --from {} --to {}{}",
        rows.len(),
        session.start_ms,
        session.end_ms,
        if session.cpu_only { " --cpu-only" } else { "" }
    );
    Ok(())
}
