use std::{env, io, path::Path, process};

fn main() {
    let code = match run() {
        Ok(code) => code,
        Err(error) => {
            println!("Tauri launcher: {error}");
            1
        }
    };
    process::exit(code);
}

fn run() -> io::Result<i32> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    let cli =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../node_modules/@tauri-apps/cli/tauri.js");
    let mut command = process::Command::new("bun");
    command.arg(cli).args(&args);

    #[cfg(unix)]
    {
        let dev = args.first().is_some_and(|arg| arg == "dev");
        unix::run(command, dev)
    }
    #[cfg(windows)]
    {
        windows::run(command)
    }
}

#[cfg(unix)]
mod unix {
    use signal_hook::{flag, low_level::pipe};
    use std::{
        env, fs,
        io::{self, Read},
        os::unix::{fs::MetadataExt, fs::PermissionsExt, process::ExitStatusExt},
        path::PathBuf,
        process::{Command, ExitStatus},
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
    };
    use tempfile::TempDir;

    pub fn run(mut command: Command, dev: bool) -> io::Result<i32> {
        let (mut wakeups, wake) = io::pipe()?;
        let signals =
            [libc::SIGINT, libc::SIGTERM].map(|signal| (signal, Arc::new(AtomicBool::new(false))));
        for (signal, pending) in &signals {
            flag::register(*signal, Arc::clone(pending))?;
            pipe::register(*signal, wake.try_clone()?)?;
        }
        pipe::register(libc::SIGCHLD, wake)?;

        let scratch = if dev {
            let dir = dev_temp_dir()?;
            command.env("TMPDIR", dir.path());
            println!("\x1b[93mThermal Berry: temporary directory isolation is active (TMPDIR).\x1b[0m");
            Some(dir)
        } else {
            None
        };

        let status = run_child(command, &mut wakeups, &signals);
        if let Some(dir) = scratch {
            if let Err(error) = dir.close() {
                if error.kind() != io::ErrorKind::NotFound {
                    return Err(error);
                }
            }
        }
        let status = status?;
        Ok(status
            .code()
            .or_else(|| status.signal().map(|signal| 128 + signal))
            .unwrap_or(1))
    }

    fn dev_temp_dir() -> io::Result<TempDir> {
        let parent = ["TMPDIR", "TMP", "TEMP"]
            .into_iter()
            .filter_map(env::var_os)
            .find(|value| !value.is_empty())
            .map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
        let metadata = fs::metadata(&parent)?;
        let uid = unsafe { libc::getuid() };
        let mode = metadata.mode();
        if !metadata.is_dir() || (metadata.uid() != 0 && metadata.uid() != uid) || (mode & 0o022 != 0 && mode & 0o1000 == 0)
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "The development temp directory must be private or have the sticky bit set.",
            ));
        }

        let permissions = fs::Permissions::from_mode(0o700);
        let dir = tempfile::Builder::new()
            .prefix("thermal-berry-dev-")
            .permissions(permissions.clone())
            .tempdir_in(parent)?;

        fs::set_permissions(dir.path(), permissions)?;
        Ok(dir)
    }

    fn run_child(
        mut command: Command,
        wakeups: &mut io::PipeReader,
        signals: &[(libc::c_int, Arc<AtomicBool>)],
    ) -> io::Result<ExitStatus> {
        let mut child = command.spawn()?;
        loop {
            if let Some(status) = child.try_wait()? {
                return Ok(status);
            }
            for (signal, pending) in signals {
                if !pending.swap(false, Ordering::Relaxed) {
                    continue;
                }
                if unsafe { libc::kill(child.id() as libc::pid_t, *signal) } == -1 {
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() != Some(libc::ESRCH) {
                        eprintln!("Tauri launcher could not forward signal {signal}: {error}");
                    }
                }
            }
            wakeups.read_exact(&mut [0])?;
        }
    }
}

#[cfg(windows)]
mod windows {
    use std::{io, process::Command};
    use windows_sys::Win32::System::Console::{
        SetConsoleCtrlHandler, CTRL_BREAK_EVENT, CTRL_C_EVENT,
    };

    pub fn run(mut command: Command) -> io::Result<i32> {
        if unsafe { SetConsoleCtrlHandler(Some(handle_console_event), 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(command.status()?.code().unwrap_or(1))
    }

    unsafe extern "system" fn handle_console_event(event: u32) -> i32 {
        i32::from(matches!(event, CTRL_C_EVENT | CTRL_BREAK_EVENT))
    }
}
