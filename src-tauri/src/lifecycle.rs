use std::sync::Mutex;

use tauri::{AppHandle, Manager};

#[derive(Default)]
struct Shutdown {
    busy: bool,
    quit_requested: bool,
}

#[derive(Default)]
pub struct Lifecycle(Mutex<Shutdown>);

pub fn quit(app: &AppHandle) {
    let lifecycle = app.state::<Lifecycle>();
    let exit = {
        let mut state = lifecycle.0.lock().unwrap();
        state.quit_requested = true;
        !state.busy
    };
    if exit {
        app.exit(0);
    }
}

pub fn defer_exit(app: &AppHandle) -> bool {
    let lifecycle = app.state::<Lifecycle>();
    let mut state = lifecycle.0.lock().unwrap();
    if state.busy {
        state.quit_requested = true;
    }
    state.busy
}

pub fn allow_settings(app: &AppHandle) -> bool {
    let state = app.state::<Lifecycle>();
    let state = state.0.lock().unwrap();
    !state.busy && !state.quit_requested
}

#[tauri::command]
pub async fn quit_app(app: AppHandle) {
    quit(&app);
}

#[cfg(target_os = "linux")]
pub struct Maintenance {
    app: AppHandle,
    finished: bool,
}

#[cfg(target_os = "linux")]
impl Maintenance {
    pub fn start(app: &AppHandle) -> Result<Self, crate::thermal::ThermalError> {
        let lifecycle = app.state::<Lifecycle>();
        let mut state = lifecycle.0.lock().unwrap();
        if state.busy || state.quit_requested {
            return Err(crate::thermal::ThermalError::Control(
                "maintenance or shutdown is already in progress".into(),
            ));
        }
        state.busy = true;
        Ok(Self {
            app: app.clone(),
            finished: false,
        })
    }

    pub fn complete(mut self, restart: bool) {
        self.finish(Some(restart));
    }

    fn finish(&mut self, restart: Option<bool>) {
        self.finished = true;
        let lifecycle = self.app.state::<Lifecycle>();
        let exit = {
            let mut state = lifecycle.0.lock().unwrap();
            state.busy = false;
            let exit = state.quit_requested;
            state.quit_requested |= restart.is_some();
            exit
        };
        match (exit, restart) {
            (true, _) | (_, Some(false)) => self.app.exit(0),
            (_, Some(true)) => self.app.request_restart(),
            (_, None) => {}
        }
    }
}

#[cfg(target_os = "linux")]
impl Drop for Maintenance {
    fn drop(&mut self) {
        if !self.finished {
            self.finish(None);
        }
    }
}
