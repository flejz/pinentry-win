use std::collections::HashMap;

use zeroize::Zeroize;

use crate::error::CommandResult;

/// All mutable state accumulated by SET* and OPTION commands.
///
/// Fields that may contain user secrets (desc, prompt, error_text) are
/// zeroed on drop via the Zeroize derive.  Dialog-specific fields are
/// cleared by `reset()`; option state (grab, no_grab, options) persists
/// across RESET because gpg-agent sets options once at session start.
#[derive(Debug, Default, Zeroize)]
pub struct PinentryState {
    // SET* fields — cleared by reset()
    pub desc: String,
    pub prompt: String,
    pub title: String,
    pub ok_button: String,
    pub cancel_button: String,
    pub notok_button: String,
    pub error_text: String,
    pub keyinfo: String,
    pub repeat_ok: bool,
    pub one_button: bool,
    pub notok_set: bool,

    // OPTION fields — persist across RESET
    pub grab: bool,
    pub no_grab: bool,
    #[zeroize(skip)]
    pub options: HashMap<String, String>,
}

impl PinentryState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear all dialog-specific state to defaults.
    /// Option state (grab / no_grab / options) is intentionally preserved
    /// because gpg-agent sends OPTION once per session, before any dialog.
    pub fn reset(&mut self) {
        self.desc.zeroize();
        self.prompt.zeroize();
        self.title.zeroize();
        self.ok_button.zeroize();
        self.cancel_button.zeroize();
        self.notok_button.zeroize();
        self.error_text.zeroize();
        self.keyinfo.zeroize();
        self.repeat_ok = false;
        self.one_button = false;
        self.notok_set = false;
    }

    /// Handle an OPTION command.
    ///
    /// Unknown option names MUST return Ok(()) — gpg-agent probes capabilities
    /// by firing options and expecting OK for anything it doesn't know about.
    pub fn apply_option(&mut self, name: &str, value: Option<&str>) -> CommandResult {
        match name {
            "grab" => self.grab = true,
            "no-grab" => self.no_grab = true,

            // Per-dialog text overrides sent via OPTION before the first dialog
            "default-ok" => {
                if let Some(v) = value {
                    if self.ok_button.is_empty() {
                        self.ok_button = v.to_owned();
                    }
                }
            }
            "default-cancel" => {
                if let Some(v) = value {
                    if self.cancel_button.is_empty() {
                        self.cancel_button = v.to_owned();
                    }
                }
            }
            "default-yes" => {
                if let Some(v) = value {
                    if self.ok_button.is_empty() {
                        self.ok_button = v.to_owned();
                    }
                }
            }
            "default-no" => {
                if let Some(v) = value {
                    if self.cancel_button.is_empty() {
                        self.cancel_button = v.to_owned();
                    }
                }
            }
            "default-prompt" => {
                if let Some(v) = value {
                    if self.prompt.is_empty() {
                        self.prompt = v.to_owned();
                    }
                }
            }

            // Known no-op options — store silently so we don't reject them.
            // gpg-agent sends these on every session; we don't use them on
            // Windows (no TTY, no locale switching needed).
            "ttyname"
            | "ttytype"
            | "lc-ctype"
            | "lc-messages"
            | "touch-file"
            | "owner"
            | "allow-external-password-cache"
            | "allow-emacs-prompt"
            | "invisible-char"
            | "default-pwmngr"
            | "formatted-passphrase"
            | "formatted-passphrase-hint"
            | "constraints-enforce"
            | "constraints-hint-short"
            | "constraints-hint-long"
            | "constraints-error-title" => {
                self.options.insert(
                    name.to_owned(),
                    value.unwrap_or("").to_owned(),
                );
            }

            // All other unknown options — store and succeed.
            _ => {
                self.options.insert(
                    name.to_owned(),
                    value.unwrap_or("").to_owned(),
                );
            }
        }
        Ok(())
    }

    // --- Display helpers with sensible fallbacks ---

    pub fn title_str(&self) -> &str {
        if self.title.is_empty() {
            "Enter Passphrase"
        } else {
            &self.title
        }
    }

    pub fn prompt_str(&self) -> &str {
        if self.prompt.is_empty() {
            "PIN:"
        } else {
            &self.prompt
        }
    }

    pub fn ok_str(&self) -> &str {
        if self.ok_button.is_empty() {
            "OK"
        } else {
            &self.ok_button
        }
    }

    pub fn cancel_str(&self) -> &str {
        if self.cancel_button.is_empty() {
            "Cancel"
        } else {
            &self.cancel_button
        }
    }

    pub fn notok_str(&self) -> &str {
        if self.notok_button.is_empty() {
            "No"
        } else {
            &self.notok_button
        }
    }

    /// Returns `Some(&str)` when a description has been set, `None` otherwise.
    pub fn desc_str(&self) -> Option<&str> {
        if self.desc.is_empty() {
            None
        } else {
            Some(&self.desc)
        }
    }

    /// Returns `Some(&str)` when an error message has been set, `None` otherwise.
    pub fn error_str(&self) -> Option<&str> {
        if self.error_text.is_empty() {
            None
        } else {
            Some(&self.error_text)
        }
    }
}
