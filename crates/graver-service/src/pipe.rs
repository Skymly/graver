//! Named-pipe transport.
//!
//! [`create_pipe`] installs an explicit DACL for the current user and rejects
//! remote clients. Do not fall back to a null descriptor: the default DACL
//! grants Everyone read access. AppContainer hosts may be unable to connect;
//! the text service returns the key instead of widening this ACL.

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use graver_engine::Engine;
use graver_ipc::{FrameDecoder, ProtocolError, decode_request, encode_response};
use windows::{
    Win32::{
        Foundation::{
            ERROR_BROKEN_PIPE, ERROR_NO_DATA, ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED,
            HANDLE,
        },
        Storage::FileSystem::{PIPE_ACCESS_DUPLEX, ReadFile, WriteFile},
        System::Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
        },
    },
    core::{Error, HRESULT, PCWSTR},
};

use crate::{ServiceError, dispatch, security::CurrentUserPipeSecurity};

pub fn serve_pipe(name: &str) -> Result<(), ServiceError> {
    eprintln!("Graver service listening on {name}");
    loop {
        let pipe = create_pipe(name)?;
        connect(&pipe)?;
        thread::spawn(move || {
            if let Err(err) = session(pipe)
                && !matches!(err, ServiceError::Disconnected)
            {
                eprintln!("graver-service: {err}");
            }
        });
    }
}

pub fn serve_pipe_once(name: &str, ready: &AtomicBool) -> Result<(), ServiceError> {
    let pipe = create_pipe(name)?;
    ready.store(true, Ordering::SeqCst);
    connect(&pipe)?;
    session(pipe)
}

fn session(pipe: OwnedHandle) -> Result<(), ServiceError> {
    let mut engine = Engine::new();
    let mut decoder = FrameDecoder::new();
    let mut buf = [0u8; 4096];
    loop {
        let read = match read_some(&pipe, &mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(ServiceError::Disconnected) => break,
            Err(err) => return Err(err),
        };
        for payload in decoder.push(&buf[..read])? {
            let request = decode_request(&payload)?;
            let response = dispatch(&mut engine, request);
            match write_all(&pipe, &encode_response(&response)?) {
                Ok(()) => {}
                Err(ServiceError::Disconnected) => return Ok(()),
                Err(err) => return Err(err),
            }
        }
    }
    if decoder.pending() > 0 {
        return Err(ProtocolError::UnexpectedEof.into());
    }
    Ok(())
}

/// Production pipe constructor. [`serve_pipe`] and [`serve_pipe_once`] are the
/// only listeners, and both call this function. There is no null-descriptor path.
fn create_pipe(name: &str) -> Result<OwnedHandle, ServiceError> {
    let mut security = CurrentUserPipeSecurity::for_current_user()?;
    let wide = wide_null(name);
    let attributes = security.attributes();
    // SAFETY: `wide` is NUL-terminated. `attributes` borrows `security`, which
    // outlives this call. The kernel copies the descriptor before returning.
    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(wide.as_ptr()),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            64 * 1024,
            64 * 1024,
            0,
            Some(&attributes),
        )
    };
    if handle.is_invalid() {
        return Err(last_error());
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(handle.0) })
}

fn connect(pipe: &OwnedHandle) -> Result<(), ServiceError> {
    match unsafe { ConnectNamedPipe(raw(pipe), None) } {
        Ok(()) => Ok(()),
        Err(err) if is_win32(&err, ERROR_PIPE_CONNECTED.0) => Ok(()),
        Err(err) => Err(ServiceError::Windows(err.to_string())),
    }
}

fn read_some(pipe: &OwnedHandle, buf: &mut [u8]) -> Result<usize, ServiceError> {
    let mut read = 0u32;
    match unsafe { ReadFile(raw(pipe), Some(buf), Some(&raw mut read), None) } {
        Ok(()) => Ok(read as usize),
        Err(err) if is_disconnect(&err) => Err(ServiceError::Disconnected),
        Err(err) => Err(ServiceError::Windows(err.to_string())),
    }
}

fn write_all(pipe: &OwnedHandle, mut data: &[u8]) -> Result<(), ServiceError> {
    while !data.is_empty() {
        let mut written = 0u32;
        match unsafe { WriteFile(raw(pipe), Some(data), Some(&raw mut written), None) } {
            Ok(()) if written == 0 => {
                return Err(ServiceError::Windows("pipe write made no progress".into()));
            }
            Ok(()) => data = &data[written as usize..],
            Err(err) if is_disconnect(&err) => return Err(ServiceError::Disconnected),
            Err(err) => return Err(ServiceError::Windows(err.to_string())),
        }
    }
    Ok(())
}

fn raw(pipe: &OwnedHandle) -> HANDLE {
    HANDLE(pipe.as_raw_handle())
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn last_error() -> ServiceError {
    ServiceError::Windows(Error::from_thread().to_string())
}

fn is_disconnect(err: &Error) -> bool {
    is_win32(err, ERROR_BROKEN_PIPE.0)
        || is_win32(err, ERROR_NO_DATA.0)
        || is_win32(err, ERROR_PIPE_NOT_CONNECTED.0)
}

fn is_win32(err: &Error, code: u32) -> bool {
    err.code() == HRESULT::from_win32(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::inspect;
    use graver_ipc::{
        KeyKindMessage, KeyMessage, PIPE_NAME, Request, Response, decode_response, encode_request,
    };
    use std::sync::atomic::AtomicU64;
    use std::time::{Duration, Instant};
    use windows::Win32::{
        Foundation::{GENERIC_READ, GENERIC_WRITE},
        Storage::FileSystem::{CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_MODE, OPEN_EXISTING},
        System::Pipes::GetNamedPipeInfo,
    };

    fn unique_pipe_name(label: &str) -> String {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        format!(r"\\.\pipe\Graver.{label}.{}.{n}", std::process::id())
    }

    #[test]
    fn ping_over_named_pipe() {
        let name = unique_pipe_name("Ping");
        assert_ne!(name, PIPE_NAME);
        let ready = AtomicBool::new(false);
        thread::scope(|scope| {
            let server = scope.spawn(|| serve_pipe_once(&name, &ready));
            let start = Instant::now();
            while !ready.load(Ordering::SeqCst) {
                if server.is_finished() {
                    panic!("server exited before listening: {:?}", server.join());
                }
                if start.elapsed() > Duration::from_secs(5) {
                    panic!("timed out waiting for {name}");
                }
                thread::sleep(Duration::from_millis(10));
            }

            let response = client_exchange(&name, &Request::Ping { v: 1, id: 1 }).unwrap();
            server.join().unwrap().unwrap();
            assert!(matches!(response, Response::Pong { id: 1, .. }));
        });
    }

    #[test]
    fn key_over_named_pipe_reaches_the_engine() {
        let name = unique_pipe_name("Key");
        assert_ne!(name, PIPE_NAME);
        let ready = AtomicBool::new(false);
        thread::scope(|scope| {
            let server = scope.spawn(|| serve_pipe_once(&name, &ready));
            let start = Instant::now();
            while !ready.load(Ordering::SeqCst) {
                if server.is_finished() {
                    panic!("server exited before listening: {:?}", server.join());
                }
                if start.elapsed() > Duration::from_secs(5) {
                    panic!("timed out waiting for {name}");
                }
                thread::sleep(Duration::from_millis(10));
            }

            let response = client_exchange(
                &name,
                &Request::Key {
                    v: 1,
                    id: 2,
                    key: KeyMessage {
                        kind: KeyKindMessage::Char { ch: 'q' },
                        shift: false,
                        ctrl: false,
                        alt: false,
                    },
                },
            )
            .unwrap();
            server.join().unwrap().unwrap();
            match response {
                Response::Update {
                    preedit,
                    consumed,
                    commit,
                    candidates,
                    ..
                } => {
                    assert_eq!(preedit, "q");
                    assert!(consumed);
                    assert!(commit.is_none());
                    assert!(candidates.is_empty());
                }
                other => panic!("unexpected response: {other:?}"),
            }
        });
    }

    #[test]
    fn production_pipe_dacl_allows_only_the_current_user() {
        let name = unique_pipe_name("Acl");
        assert_ne!(name, PIPE_NAME);
        assert!(name.starts_with(r"\\.\pipe\Graver."));
        // Same constructor the production listener calls. No graver-service process.
        let pipe = create_pipe(&name).unwrap();
        let dacl = inspect::read_allow_entries(raw(&pipe)).unwrap();
        let user = inspect::current_user_sid_bytes().unwrap();
        assert!(
            !dacl.allow.is_empty(),
            "DACL does not allow the current user"
        );
        assert!(
            dacl.allow.iter().all(|sid| inspect::sid_equal(sid, &user)),
            "allow list is not only the current user: {:?}",
            dacl.allow_strings
        );

        let forbidden = inspect::forbidden_sids().unwrap();
        for entry in &forbidden {
            assert_eq!(
                inspect::sid_string(&entry.bytes).unwrap(),
                entry.string_sid,
                "{} well-known SID did not match {}",
                entry.label,
                entry.string_sid
            );
            assert!(
                dacl.allow
                    .iter()
                    .all(|allow| !inspect::sid_equal(allow, &entry.bytes)),
                "{} ({}) is in the allow list: {:?}",
                entry.label,
                entry.string_sid,
                dacl.allow_strings
            );
            assert!(
                !dacl.allow_strings.iter().any(|got| got == entry.string_sid),
                "{} ({}) string is in the allow list: {:?}",
                entry.label,
                entry.string_sid,
                dacl.allow_strings
            );
        }

        let mut flags = PIPE_TYPE_BYTE;
        // SAFETY: `pipe` is a live named-pipe handle created by `create_pipe`.
        unsafe {
            GetNamedPipeInfo(raw(&pipe), Some(&mut flags), None, None, None).unwrap();
        }
        assert!(
            flags.contains(PIPE_REJECT_REMOTE_CLIENTS),
            "PIPE_REJECT_REMOTE_CLIENTS missing from {flags:?}"
        );

        // Same-user client can open the restricted pipe. Close both handles here.
        let client = open_client(&name).unwrap();
        drop(client);
        drop(pipe);
    }

    #[test]
    fn handwritten_key_frame_reaches_the_engine() {
        let name = unique_pipe_name("Golden");
        assert_ne!(name, PIPE_NAME);
        let ready = AtomicBool::new(false);
        thread::scope(|scope| {
            let server = scope.spawn(|| serve_pipe_once(&name, &ready));
            let start = Instant::now();
            while !ready.load(Ordering::SeqCst) {
                if server.is_finished() {
                    panic!("server exited before listening: {:?}", server.join());
                }
                if start.elapsed() > Duration::from_secs(5) {
                    panic!("timed out waiting for {name}");
                }
                thread::sleep(Duration::from_millis(10));
            }

            let payload = br#"{"op":"key","v":1,"id":4,"key":{"kind":"char","ch":"a","shift":false,"ctrl":false,"alt":false}}"#;
            let response = exchange_payload(&name, payload).unwrap();
            server.join().unwrap().unwrap();
            match response {
                Response::Update {
                    id,
                    preedit,
                    consumed,
                    commit,
                    ..
                } => {
                    assert_eq!(id, 4);
                    assert_eq!(preedit, "a");
                    assert!(consumed);
                    assert!(commit.is_none());
                }
                other => panic!("unexpected response: {other:?}"),
            }
        });
    }

    #[test]
    fn settings_probe_frame_still_reaches_the_listener() {
        let name = unique_pipe_name("Probe");
        assert_ne!(name, PIPE_NAME);
        let ready = AtomicBool::new(false);
        thread::scope(|scope| {
            let server = scope.spawn(|| serve_pipe_once(&name, &ready));
            let start = Instant::now();
            while !ready.load(Ordering::SeqCst) {
                if server.is_finished() {
                    panic!("server exited before listening: {:?}", server.join());
                }
                if start.elapsed() > Duration::from_secs(5) {
                    panic!("timed out waiting for {name}");
                }
                thread::sleep(Duration::from_millis(10));
            }

            let response = exchange_payload(&name, br#"{"v":1,"id":1,"op":"ping"}"#).unwrap();
            server.join().unwrap().unwrap();
            assert!(matches!(response, Response::Pong { id: 1, .. }));
        });
    }
    fn open_client(name: &str) -> Result<OwnedHandle, ServiceError> {
        let wide = wide_null(name);
        // SAFETY: `wide` is NUL-terminated. The returned handle is owned by the caller.
        let handle = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_MODE(0),
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        }
        .map_err(|err| ServiceError::Windows(err.to_string()))?;
        Ok(unsafe { OwnedHandle::from_raw_handle(handle.0) })
    }

    fn client_exchange(name: &str, request: &Request) -> Result<Response, ServiceError> {
        let pipe = open_client(name)?;
        write_all(&pipe, &encode_request(request)?)?;
        read_response(&pipe)
    }

    fn exchange_payload(name: &str, payload: &[u8]) -> Result<Response, ServiceError> {
        let mut frame = Vec::with_capacity(4 + payload.len());
        frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        frame.extend_from_slice(payload);
        let pipe = open_client(name)?;
        write_all(&pipe, &frame)?;
        read_response(&pipe)
    }
    fn read_response(pipe: &OwnedHandle) -> Result<Response, ServiceError> {
        let mut decoder = FrameDecoder::new();
        let mut buf = [0u8; 4096];
        loop {
            let read = read_some(pipe, &mut buf)?;
            if read == 0 {
                return Err(ProtocolError::UnexpectedEof.into());
            }
            if let Some(payload) = decoder.push(&buf[..read])?.into_iter().next() {
                return decode_response(&payload).map_err(Into::into);
            }
        }
    }
}
