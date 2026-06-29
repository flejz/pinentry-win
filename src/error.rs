use std::fmt;

// GPG error composition: (source & 0x7F) << 24 | (code & 0xFFFF)
// GPG_ERR_SOURCE_PINENTRY = 5
// These are the pre-packed values matching libgpg-error constants.
pub const GPG_ERR_CANCELED: u32 = 83886179u32;      // (5 << 24) | 99
pub const GPG_ERR_NOT_CONFIRMED: u32 = 83886258u32;  // (5 << 24) | 114
pub const GPG_ERR_ASS_UNKNOWN_CMD: u32 = 83886419u32; // (4 << 24) | 275

// Additional useful code (general Assuan error)
pub const GPG_ERR_ASS_GENERAL: u32 = 83886337u32;   // (5 << 24) | 257

pub type CommandResult = Result<(), AssuanError>;

#[derive(Debug)]
pub struct AssuanError {
    pub code: u32,
    pub message: String,
}

impl AssuanError {
    pub fn new(code: u32, message: impl Into<String>) -> Self {
        AssuanError {
            code,
            message: message.into(),
        }
    }

    pub fn canceled() -> Self {
        AssuanError::new(GPG_ERR_CANCELED, gpg_error_string(GPG_ERR_CANCELED))
    }

    pub fn not_confirmed() -> Self {
        AssuanError::new(GPG_ERR_NOT_CONFIRMED, gpg_error_string(GPG_ERR_NOT_CONFIRMED))
    }

    pub fn unknown_cmd() -> Self {
        AssuanError::new(GPG_ERR_ASS_UNKNOWN_CMD, gpg_error_string(GPG_ERR_ASS_UNKNOWN_CMD))
    }
}

impl fmt::Display for AssuanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "GPG error {}: {}", self.code, self.message)
    }
}

impl std::error::Error for AssuanError {}

impl From<std::io::Error> for AssuanError {
    fn from(e: std::io::Error) -> Self {
        AssuanError::new(GPG_ERR_ASS_GENERAL, e.to_string())
    }
}

/// Returns the text description used in "ERR <code> <description>" lines.
pub fn gpg_error_string(code: u32) -> &'static str {
    match code {
        GPG_ERR_CANCELED => "canceled",
        GPG_ERR_NOT_CONFIRMED => "not confirmed",
        GPG_ERR_ASS_UNKNOWN_CMD => "unknown IPC command",
        GPG_ERR_ASS_GENERAL => "general IPC error",
        _ => "general error",
    }
}
