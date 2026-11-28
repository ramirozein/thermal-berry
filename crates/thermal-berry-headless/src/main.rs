use std::{
    env,
    fs::File,
    io::{BufRead, BufReader, Read},
    path::Path,
    process::ExitCode,
};
use thermal_berry_core::{
    identify::{identify, Report, MAX_SAMPLES},
    observation::Observation,
};

#[cfg(target_os = "linux")]
mod linux;

const USAGE: &str = "thermal-berry-headless calibrate [--check] [--cpu-only]
thermal-berry-headless identify --from START_MS --to END_MS [--cpu-only]
thermal-berry-headless identify --input SESSION.jsonl [--cpu-only]";

fn main() -> ExitCode {
    let args: Vec<_> = env::args().skip(1).collect();
    if matches!(args.as_slice(), [flag] if flag == "--help" || flag == "-h") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if args.is_empty() {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    }
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            if matches!(
                e.downcast_ref::<thermal_berry_core::lock::LockError>(),
                Some(thermal_berry_core::lock::LockError::Busy(_))
            ) {
                return ExitCode::from(2);
            }
            ExitCode::FAILURE
        }
    }
}

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn run(mut args: Vec<String>) -> Result<()> {
    let cpu_only = if let Some(i) = args.iter().position(|f| f == "--cpu-only") {
        args.remove(i);
        true
    } else {
        false
    };
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let _instance = thermal_berry_core::lock::InstanceLock::acquire("headless")?;
    match args.as_slice() {
        ["identify", "--input", path] => {
            let rows = read_session(Path::new(path))?;
            print_report(&identify(&rows, cpu_only)?)
        }
        #[cfg(target_os = "linux")]
        ["identify", "--from", start, "--to", end] => {
            let (from, to) = (start.parse::<u64>()?, end.parse::<u64>()?);
            if from >= to || to > i64::MAX as u64 {
                return Err("invalid calibration time range".into());
            }
            let db = thermal_berry_core::db::Db::open()?;
            let report = identify(&db.observations(from, to)?, cpu_only)?;
            print_report(&report)?;
            db.save_identification(&report)?;
            Ok(())
        }
        #[cfg(target_os = "linux")]
        ["calibrate"] => linux::calibrate(cpu_only, false),
        #[cfg(target_os = "linux")]
        ["calibrate", "--check"] => linux::calibrate(cpu_only, true),
        #[cfg(not(target_os = "linux"))]
        ["calibrate", ..] | ["identify", "--from", ..] => {
            Err("Hardware calibration and SQLite sessions require Linux. Use identify --input for offline analysis.".into())
        }
        _ => Err(USAGE.into()),
    }
}

fn print_report(report: &Report) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(report)?);
    if !report.accepted {
        return Err("session did not pass the M1 acceptance criteria".into());
    }
    Ok(())
}

fn read_session(path: &Path) -> Result<Vec<Observation>> {
    const LIMIT: u64 = 16 * 1024 * 1024;
    let mut options = File::options();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > LIMIT {
        return Err("expected a local JSONL file up to 16 MiB".into());
    }
    let mut reader = BufReader::new(file.take(LIMIT + 1));
    let mut rows = Vec::new();
    let mut line = String::new();
    while (&mut reader).take(4097).read_line(&mut line)? > 0 {
        if rows.len() >= MAX_SAMPLES || line.len() > 4096 {
            return Err("calibration input exceeds row limits".into());
        }
        rows.push(serde_json::from_str(&line).map_err(|e| format!("row {}: {e}", rows.len() + 1))?);
        line.clear();
    }
    if reader.get_ref().limit() == 0 {
        return Err("session file exceeds size limit".into());
    }
    Ok(rows)
}
