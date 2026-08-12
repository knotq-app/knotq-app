//! Windows single-instance coordination for protocol activation.

use serde::{Deserialize, Serialize};

const MAX_ACTIVATION_BYTES: usize = 64 * 1024;

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ActivationEnvelope {
    args: Vec<String>,
}

fn encode_activation(args: Vec<String>) -> Option<Vec<u8>> {
    let bytes = serde_json::to_vec(&ActivationEnvelope { args }).ok()?;
    (bytes.len() <= MAX_ACTIVATION_BYTES).then_some(bytes)
}

fn decode_activation(bytes: &[u8]) -> Option<Vec<String>> {
    if bytes.len() > MAX_ACTIVATION_BYTES {
        return None;
    }
    serde_json::from_slice::<ActivationEnvelope>(bytes)
        .ok()
        .map(|envelope| envelope.args)
}

#[cfg(windows)]
mod platform {
    use super::{decode_activation, encode_activation, MAX_ACTIVATION_BYTES};
    use std::env;
    use std::thread;
    use std::time::{Duration, Instant};
    use windows::core::w;
    use windows::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, ERROR_PIPE_CONNECTED, GENERIC_WRITE,
    };
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, ReadFile, WriteFile, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_NONE, OPEN_EXISTING,
        PIPE_ACCESS_INBOUND,
    };
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, WaitNamedPipeW,
        PIPE_READMODE_MESSAGE, PIPE_TYPE_MESSAGE, PIPE_WAIT,
    };
    use windows::Win32::System::Threading::CreateMutexW;

    const PIPE_BUFFER_BYTES: u32 = MAX_ACTIVATION_BYTES as u32;

    /// Returns true in a secondary process after forwarding its activation to
    /// the primary process.  The caller must return from `main` immediately.
    pub fn run_secondary_from_env() -> bool {
        let mutex = match unsafe { CreateMutexW(None, false, w!("Local\\KnotQ.SingleInstance.v1")) }
        {
            Ok(handle) => handle,
            Err(_) => return false,
        };
        let already_running = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        if already_running {
            let args = env::args().skip(1).collect();
            let _ = forward_to_primary(args);
            let _ = unsafe { CloseHandle(mutex) };
            return true;
        }

        // A Win32 HANDLE has no Rust destructor. Deliberately leave this handle
        // open for the process lifetime so the named mutex continues to exist.
        let _primary_mutex = mutex;
        thread::Builder::new()
            .name("knotq-windows-activation".into())
            .spawn(activation_server)
            .is_err()
            .then(|| eprintln!("failed to start Windows activation handoff server"));
        false
    }

    fn forward_to_primary(args: Vec<String>) -> Option<()> {
        let bytes = encode_activation(args)?;
        // The mutex is visible slightly before the primary creates its pipe.
        // Retry both "not created yet" and "current instance busy" states;
        // WaitNamedPipe alone returns immediately when the pipe does not exist.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if unsafe { WaitNamedPipeW(w!(r"\\.\pipe\KnotQ.Activation.v1"), 100) }.as_bool() {
                if let Ok(pipe) = unsafe {
                    CreateFileW(
                        w!(r"\\.\pipe\KnotQ.Activation.v1"),
                        GENERIC_WRITE.0,
                        FILE_SHARE_NONE,
                        None,
                        OPEN_EXISTING,
                        FILE_ATTRIBUTE_NORMAL,
                        None,
                    )
                } {
                    let result = unsafe { WriteFile(pipe, Some(&bytes), None, None) }.ok();
                    let _ = unsafe { CloseHandle(pipe) };
                    return result;
                }
            }
            if Instant::now() >= deadline {
                return None;
            }
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn activation_server() {
        loop {
            let pipe = unsafe {
                CreateNamedPipeW(
                    w!(r"\\.\pipe\KnotQ.Activation.v1"),
                    PIPE_ACCESS_INBOUND,
                    PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT,
                    1,
                    PIPE_BUFFER_BYTES,
                    PIPE_BUFFER_BYTES,
                    0,
                    None,
                )
            };
            if pipe.is_invalid() {
                return;
            }
            let connected = unsafe { ConnectNamedPipe(pipe, None) }.is_ok()
                || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
            if connected {
                let mut bytes = vec![0; MAX_ACTIVATION_BYTES];
                let mut read = 0;
                if unsafe { ReadFile(pipe, Some(&mut bytes), Some(&mut read), None) }.is_ok() {
                    bytes.truncate(read as usize);
                    if let Some(args) = decode_activation(&bytes) {
                        crate::platform_provider::dispatch_windows_activation_args(args);
                    }
                }
            }
            let _ = unsafe { DisconnectNamedPipe(pipe) };
            let _ = unsafe { CloseHandle(pipe) };
        }
    }
}

#[cfg(windows)]
pub use platform::run_secondary_from_env;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_handoff_preserves_uri_as_one_argument() {
        let args =
            vec!["knotq://notification?action_id=knotq.mark_done&capability=a%26b".to_string()];
        assert_eq!(
            decode_activation(&encode_activation(args.clone()).unwrap()),
            Some(args)
        );
    }

    #[test]
    fn activation_handoff_rejects_oversized_messages() {
        assert!(encode_activation(vec!["x".repeat(MAX_ACTIVATION_BYTES)]).is_none());
        assert!(decode_activation(&vec![b'x'; MAX_ACTIVATION_BYTES + 1]).is_none());
    }
}
