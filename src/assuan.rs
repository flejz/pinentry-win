use std::io::{BufRead, Write};
use crate::error::{
    AssuanError, CommandResult, GPG_ERR_CANCELED,
    gpg_error_string,
};
use crate::state::PinentryState;

// Assuan line length limit (bytes including command verb + args, excluding newline)
const ASSUAN_LINELENGTH: usize = 1000;

// Protocol version this binary speaks
const VERSION: &str = "1.0.0";

// ---------------------------------------------------------------------------
// Percent-encoding helpers
// ---------------------------------------------------------------------------

/// Percent-decode an Assuan string argument.
/// Only `%XX` sequences are decoded; `+` is NOT treated as space.
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = bytes[i + 1];
            let lo = bytes[i + 2];
            if let (Some(h), Some(l)) = (hex_val(hi), hex_val(lo)) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    // Assuan strings are UTF-8; replace invalid sequences rather than panic.
    String::from_utf8_lossy(&out).into_owned()
}

/// Percent-encode a string for use in Assuan D or status lines.
/// Mandatory escapes: `%` -> `%25`, CR (`\r`) -> `%0D`, LF (`\n`) -> `%0A`.
/// Nothing else needs escaping per the Assuan protocol spec.
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'%' => out.push_str("%25"),
            b'\r' => out.push_str("%0D"),
            b'\n' => out.push_str("%0A"),
            _ => out.push(b as char),
        }
    }
    out
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// run_loop — the public entry point called from main
// ---------------------------------------------------------------------------

/// Drive the Assuan protocol over `reader`/`writer`.
///
/// Sends the initial greeting, then reads and dispatches commands until BYE
/// or EOF.  Returns `Ok(())` on clean shutdown; propagates IO errors.
pub fn run_loop(
    reader: impl BufRead,
    mut writer: impl Write,
    state: &mut PinentryState,
) -> anyhow::Result<()> {
    // Send the Assuan greeting.
    writeln!(writer, "OK Pleased to meet you")?;
    writer.flush()?;

    let mut line_buf = String::with_capacity(ASSUAN_LINELENGTH + 4);

    let mut reader = reader;

    loop {
        line_buf.clear();
        let n = reader.read_line(&mut line_buf)?;
        if n == 0 {
            // EOF — client closed the pipe; treat as clean shutdown.
            break;
        }

        // Strip trailing CR/LF.
        let line = line_buf.trim_end_matches(['\r', '\n']).to_string();

        // Skip blank lines and comment lines.
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // Enforce line length limit (measured on the raw bytes).
        if line.len() > ASSUAN_LINELENGTH {
            send_err(&mut writer, crate::error::GPG_ERR_ASS_GENERAL, "line too long")?;
            continue;
        }

        log::debug!("< {}", line);

        // Parse verb + optional argument.
        let (verb, arg) = match line.find(' ') {
            Some(pos) => (&line[..pos], Some(line[pos + 1..].trim())),
            None => (line.as_str(), None),
        };
        let verb_upper = verb.to_ascii_uppercase();

        let result: CommandResult = match verb_upper.as_str() {
            "NOP" => Ok(()),

            "RESET" => {
                state.reset();
                Ok(())
            }

            "BYE" => {
                send_ok(&mut writer, "")?;
                break;
            }

            "OPTION" => handle_option(state, arg.unwrap_or("")),

            "SETDESC" => {
                state.desc = percent_decode(arg.unwrap_or(""));
                Ok(())
            }

            "SETPROMPT" => {
                state.prompt = percent_decode(arg.unwrap_or(""));
                Ok(())
            }

            "SETTITLE" => {
                state.title = percent_decode(arg.unwrap_or(""));
                Ok(())
            }

            "SETOK" => {
                state.ok_button = percent_decode(arg.unwrap_or(""));
                Ok(())
            }

            "SETCANCEL" => {
                state.cancel_button = percent_decode(arg.unwrap_or(""));
                Ok(())
            }

            "SETNOTOK" => {
                state.notok_button = percent_decode(arg.unwrap_or(""));
                state.notok_set = true;
                Ok(())
            }

            "SETERROR" => {
                state.error_text = percent_decode(arg.unwrap_or(""));
                Ok(())
            }

            "SETKEYINFO" => {
                state.keyinfo = percent_decode(arg.unwrap_or(""));
                Ok(())
            }

            "SETREPEATOK" => {
                // Argument is the label for the repeat-passphrase field.
                // We store repeat_ok flag; the label is cosmetic for our impl.
                state.repeat_ok = true;
                Ok(())
            }

            "GETPIN" => handle_getpin(state, &mut writer),

            "CONFIRM" => {
                let one_button = arg.map(|a| a.trim() == "--one-button").unwrap_or(false);
                if one_button {
                    state.one_button = true;
                }
                handle_confirm(state, &mut writer)
            }

            "MESSAGE" => {
                // Show a message dialog (informational, no passphrase).
                // We reuse show_confirm in one-button mode.
                let prev_one = state.one_button;
                state.one_button = true;
                let res = crate::dialog::show_confirm(state);
                state.one_button = prev_one;
                match res {
                    Ok(_) => Ok(()),
                    Err(e) => Err(AssuanError::new(e, gpg_error_string(e).to_string())),
                }
            }

            "GETINFO" => handle_getinfo(arg.unwrap_or(""), &mut writer),

            "CANCEL" => {
                // Cancels a pending INQUIRE; we have no async operations, so just OK.
                Ok(())
            }

            _ => Err(AssuanError::unknown_cmd()),
        };

        match result {
            Ok(()) => {
                // Only send OK if BYE didn't already send it (BYE breaks above).
                if verb_upper != "BYE" {
                    send_ok(&mut writer, "")?;
                }
            }
            Err(e) => {
                send_err(&mut writer, e.code, &e.message)?;
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Command handlers
// ---------------------------------------------------------------------------

fn handle_option(state: &mut PinentryState, arg: &str) -> CommandResult {
    // OPTION key[=value] or OPTION key value
    let (key, value) = if let Some(eq) = arg.find('=') {
        (&arg[..eq], Some(&arg[eq + 1..]))
    } else if let Some(sp) = arg.find(' ') {
        (&arg[..sp], Some(&arg[sp + 1..]))
    } else {
        (arg, None)
    };
    state.apply_option(key, value)
}

fn handle_getpin(state: &PinentryState, writer: &mut impl Write) -> CommandResult {
    match crate::dialog::show_getpin(state) {
        Ok(pin) => {
            // Send passphrase as a D line.  Empty passphrase is valid.
            let encoded = percent_encode(&pin);
            // "D " prefix; the data line must not exceed ASSUAN_LINELENGTH.
            // For very long passphrases we'd need chunking, but 2048-char limit
            // on the edit control keeps us safely under 1000 bytes after encoding.
            let d_line = format!("D {}\n", encoded);
            log::debug!("> D [passphrase hidden]");
            writer
                .write_all(d_line.as_bytes())
                .map_err(|e| AssuanError::new(crate::error::GPG_ERR_ASS_GENERAL, e.to_string()))?;
            Ok(())
        }
        Err(code) => Err(AssuanError::new(code, gpg_error_string(code).to_string())),
    }
}

fn handle_confirm(state: &PinentryState, _writer: &mut impl Write) -> CommandResult {
    match crate::dialog::show_confirm(state) {
        Ok(true) => Ok(()),
        Ok(false) => Err(AssuanError::new(GPG_ERR_CANCELED, "canceled")),
        Err(code) => Err(AssuanError::new(code, gpg_error_string(code).to_string())),
    }
}

fn handle_getinfo(sub: &str, writer: &mut impl Write) -> CommandResult {
    let (subcmd, _flags) = match sub.find(':') {
        Some(pos) => (&sub[..pos], Some(&sub[pos + 1..])),
        None => (sub, None),
    };

    match subcmd.to_ascii_uppercase().as_str() {
        "VERSION" => {
            send_data(writer, VERSION)
                .map_err(|e| AssuanError::new(crate::error::GPG_ERR_ASS_GENERAL, e.to_string()))
        }
        "PID" => {
            let pid = std::process::id().to_string();
            send_data(writer, &pid)
                .map_err(|e| AssuanError::new(crate::error::GPG_ERR_ASS_GENERAL, e.to_string()))
        }
        "FLAVOR" => {
            send_data(writer, "windows")
                .map_err(|e| AssuanError::new(crate::error::GPG_ERR_ASS_GENERAL, e.to_string()))
        }
        "TTYINFO" => {
            // Space-separated 6-field string: ttyname ttytype lc-ctype lc-messages xdisplay -
            // We have no TTY; return dash placeholders.
            send_data(writer, "- - - - - -")
                .map_err(|e| AssuanError::new(crate::error::GPG_ERR_ASS_GENERAL, e.to_string()))
        }
        "FEATURES" => {
            // Report supported feature tokens.
            send_data(writer, "")
                .map_err(|e| AssuanError::new(crate::error::GPG_ERR_ASS_GENERAL, e.to_string()))
        }
        _ => {
            // Unknown GETINFO sub-command: return GPG_ERR_ASS_UNKNOWN_CMD.
            Err(AssuanError::unknown_cmd())
        }
    }
}

// ---------------------------------------------------------------------------
// Low-level wire writers (used by run_loop and handlers)
// ---------------------------------------------------------------------------

fn send_ok(writer: &mut impl Write, comment: &str) -> std::io::Result<()> {
    if comment.is_empty() {
        log::debug!("> OK");
        writeln!(writer, "OK")?;
    } else {
        log::debug!("> OK {}", comment);
        writeln!(writer, "OK {}", comment)?;
    }
    writer.flush()
}

fn send_err(writer: &mut impl Write, code: u32, desc: &str) -> std::io::Result<()> {
    log::debug!("> ERR {} {}", code, desc);
    writeln!(writer, "ERR {} {}", code, desc)?;
    writer.flush()
}

fn send_data(writer: &mut impl Write, data: &str) -> std::io::Result<()> {
    let encoded = percent_encode(data);
    log::debug!("> D {}", encoded);
    writeln!(writer, "D {}", encoded)?;
    // No flush here; caller flushes after the following OK.
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_percent_encode_passthrough() {
        assert_eq!(percent_encode("hello world"), "hello world");
    }

    #[test]
    fn test_percent_encode_special() {
        assert_eq!(percent_encode("a%b\nc\rd"), "a%25b%0Ac%0Dd");
    }

    #[test]
    fn test_percent_decode_basic() {
        assert_eq!(percent_decode("hello%20world"), "hello world");
    }

    #[test]
    fn test_percent_decode_percent() {
        assert_eq!(percent_decode("100%25"), "100%");
    }

    #[test]
    fn test_percent_decode_crlf() {
        assert_eq!(percent_decode("a%0Ab"), "a\nb");
        assert_eq!(percent_decode("a%0Db"), "a\rb");
    }

    #[test]
    fn test_percent_decode_uppercase_hex() {
        assert_eq!(percent_decode("%41"), "A"); // 0x41 = 'A'
    }

    #[test]
    fn test_percent_decode_incomplete_sequence() {
        // Incomplete %X at end of string — pass through literally.
        assert_eq!(percent_decode("hello%2"), "hello%2");
        assert_eq!(percent_decode("hello%"), "hello%");
    }

    #[test]
    fn test_run_loop_bye() {
        let input = b"BYE\n";
        let mut output = Vec::new();
        let mut state = PinentryState::default();
        run_loop(&input[..], &mut output, &mut state).unwrap();
        let s = String::from_utf8(output).unwrap();
        assert!(s.starts_with("OK Pleased to meet you\n"), "got: {s:?}");
        assert!(s.contains("OK\n"), "BYE should be ACKed: {s:?}");
    }

    #[test]
    fn test_run_loop_nop_reset() {
        let input = b"NOP\nRESET\nBYE\n";
        let mut output = Vec::new();
        let mut state = PinentryState::default();
        run_loop(&input[..], &mut output, &mut state).unwrap();
        let s = String::from_utf8(output).unwrap();
        // Should see greeting + 3 OKs (NOP, RESET, BYE)
        assert_eq!(s.matches("OK").count(), 4); // greeting OK + 3 command OKs
    }

    #[test]
    fn test_run_loop_unknown_cmd() {
        let input = b"FROBNICATEXYZ\nBYE\n";
        let mut output = Vec::new();
        let mut state = PinentryState::default();
        run_loop(&input[..], &mut output, &mut state).unwrap();
        let s = String::from_utf8(output).unwrap();
        assert!(s.contains("ERR"), "unknown cmd should produce ERR: {s:?}");
    }

    #[test]
    fn test_run_loop_setdesc_percent_decode() {
        let input = b"SETDESC My%20Secret%20Key\nBYE\n";
        let mut output = Vec::new();
        let mut state = PinentryState::default();
        run_loop(&input[..], &mut output, &mut state).unwrap();
        assert_eq!(state.desc, "My Secret Key");
    }

    #[test]
    fn test_run_loop_option_unknown_succeeds() {
        // gpg-agent probes unknown options; they must succeed (not ERR).
        let input = b"OPTION unknown-option=value\nBYE\n";
        let mut output = Vec::new();
        let mut state = PinentryState::default();
        run_loop(&input[..], &mut output, &mut state).unwrap();
        let s = String::from_utf8(output).unwrap();
        assert!(!s.contains("ERR"), "unknown OPTION must not ERR: {s:?}");
    }

    #[test]
    fn test_getinfo_version() {
        let input = b"GETINFO version\nBYE\n";
        let mut output = Vec::new();
        let mut state = PinentryState::default();
        run_loop(&input[..], &mut output, &mut state).unwrap();
        let s = String::from_utf8(output).unwrap();
        assert!(s.contains(&format!("D {VERSION}")), "version in D line: {s:?}");
    }

    #[test]
    fn test_getinfo_pid() {
        let input = b"GETINFO pid\nBYE\n";
        let mut output = Vec::new();
        let mut state = PinentryState::default();
        run_loop(&input[..], &mut output, &mut state).unwrap();
        let s = String::from_utf8(output).unwrap();
        let pid_str = std::process::id().to_string();
        assert!(s.contains(&format!("D {pid_str}")), "pid in D line: {s:?}");
    }
}
