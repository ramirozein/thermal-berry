use std::{env, fs, ops::Range, process::ExitCode};

const FILES: [&str; 4] = [
    "src-tauri/Cargo.toml",
    "package.json",
    "src-tauri/tauri.conf.json",
    "src-tauri/Cargo.lock",
];

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.len() != 1 || !valid_version(&args[0]) {
        return Err("Usage: bump-version MAJOR.MINOR.PATCH (e.g. 1.2.0)".into());
    }
    let version = &args[0];

    let mut updates = Vec::new();
    for path in FILES {
        let mut content = fs::read_to_string(path).map_err(|error| format!("{path}: {error}"))?;
        let range = version_range(&content, path)?;
        let previous = content[range.clone()].to_owned();
        content.replace_range(range, version);
        updates.push((path, content, previous));
    }

    for (path, content, previous) in updates {
        if previous == *version {
            println!("{path}: {version} (unchanged)");
        } else {
            fs::write(path, content).map_err(|error| format!("{path}: {error}"))?;
            println!("{path}: {previous} -> {version}");
        }
    }
    Ok(())
}

fn valid_version(version: &str) -> bool {
    let parts: Vec<_> = version.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.bytes().all(|byte| byte.is_ascii_digit())
                && (part.len() == 1 || !part.starts_with('0'))
                && part.parse::<u32>().is_ok()
        })
}

fn version_range(content: &str, path: &str) -> Result<Range<usize>, String> {
    let mut offset = 0;
    let mut section = "";
    let mut target_package = false;
    let mut ranges = Vec::new();

    for line in content.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            section = trimmed;
            target_package = false;
        }
        if section == "[[package]]" && trimmed == "name = \"thermal-berry\"" {
            target_package = true;
        }

        let value = match path {
            "src-tauri/Cargo.toml" if section == "[package]" => line
                .trim_start()
                .strip_prefix("version")
                .and_then(|rest| rest.trim_start().strip_prefix('=')),
            "src-tauri/Cargo.lock" if target_package => line
                .trim_start()
                .strip_prefix("version")
                .and_then(|rest| rest.trim_start().strip_prefix('=')),
            "package.json" | "src-tauri/tauri.conf.json" => line
                .strip_prefix("  \"version\"")
                .and_then(|rest| rest.trim_start().strip_prefix(':')),
            _ => None,
        };

        if let Some(value) = value {
            let value = value.trim_start();
            let previous = value
                .strip_prefix('"')
                .and_then(|rest| rest.split_once('"'))
                .map(|(previous, _)| previous)
                .ok_or_else(|| format!("{path}: expected a quoted version"))?;
            if !valid_version(previous) {
                return Err(format!("{path}: invalid existing version {previous:?}"));
            }
            let start = offset + line.len() - value.len() + 1;
            ranges.push(start..start + previous.len());
        }
        offset += line.len();
    }

    if ranges.len() != 1 {
        return Err(format!(
            "{path}: expected exactly one project version in the existing file layout, found {}",
            ranges.len()
        ));
    }
    Ok(ranges.remove(0))
}


// rustc scripts/bump-version.rs -o /tmp/thermal-berry-bump-version && /tmp/thermal-berry-bump-version 1.1.4