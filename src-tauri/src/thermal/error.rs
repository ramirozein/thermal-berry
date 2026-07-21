use std::io;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum ThermalError {
    #[error("no supported thermal device was found on this system")]
    DeviceNotFound,

    #[error("unknown vendor \"{0}\"")]
    UnknownVendor(String),

    #[error("this device does not support the requested operation")]
    Unsupported,

    #[error("permission denied writing to {path}")]
    PermissionDenied { path: String },

    #[error("invalid value: {0}")]
    InvalidValue(String),

    #[error("I/O error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: io::Error,
    },

    #[error("database error: {0}")]
    Database(String),
}

impl ThermalError {
    /// Wraps an io::Error, promoting EACCES/EPERM to `PermissionDenied` so the
    /// UI can offer the udev-rule fix instead of a generic failure message.
    pub fn from_io(path: &Path, source: io::Error) -> Self {
        let path = path.display().to_string();
        if source.kind() == io::ErrorKind::PermissionDenied {
            ThermalError::PermissionDenied { path }
        } else {
            ThermalError::Io { path, source }
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            ThermalError::DeviceNotFound => "device_not_found",
            ThermalError::UnknownVendor(_) => "unknown_vendor",
            ThermalError::Unsupported => "unsupported",
            ThermalError::PermissionDenied { .. } => "permission_denied",
            ThermalError::InvalidValue(_) => "invalid_value",
            ThermalError::Io { .. } => "io",
            ThermalError::Database(_) => "database",
        }
    }
}

impl serde::Serialize for ThermalError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("ThermalError", 2)?;
        s.serialize_field("kind", self.kind())?;
        s.serialize_field("message", &self.to_string())?;
        s.end()
    }
}
