//! Synchronous named-pipe client. Reads and writes time out at [`IO_TIMEOUT_MS`].
//!
//! Do not call this from `DllMain`, and do not start an async runtime. A missing
//! service, access denial, timeout, or protocol error is returned to the caller
//! so the text service can give the key back to the host.

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use windows::{
    Win32::{
        Foundation::{
            ERROR_ACCESS_DENIED, ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_IO_PENDING,
            ERROR_NO_DATA, ERROR_PIPE_BUSY, ERROR_PIPE_NOT_CONNECTED, HANDLE, WAIT_OBJECT_0,
            WAIT_TIMEOUT,
        },
        Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OVERLAPPED, FILE_SHARE_MODE,
            OPEN_EXISTING, ReadFile, WriteFile,
        },
        System::{
            IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
            Pipes::WaitNamedPipeW,
            Threading::{CreateEventW, WaitForSingleObject},
        },
    },
    core::{Error, HRESULT, PCWSTR},
};

use crate::protocol::{
    self, ClientRequest, ClientResponse, CompositionUpdate, KeyRequest, ProtocolError,
};

/// Upper bound for one pipe read or write wait. Not a connection retry budget.
pub const IO_TIMEOUT_MS: u32 = 100;
const _: () = assert!(IO_TIMEOUT_MS <= 100);

#[derive(Debug)]
pub enum ClientError {
    Protocol(ProtocolError),
    Timeout,
    Unavailable,
    Disconnected,
    Rejected(String),
    Windows(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Protocol(err) => write!(f, "protocol error: {err:?}"),
            Self::Timeout => f.write_str("timed out"),
            Self::Unavailable => f.write_str("service unavailable"),
            Self::Disconnected => f.write_str("disconnected"),
            Self::Rejected(message) => write!(f, "rejected: {message}"),
            Self::Windows(message) => write!(f, "windows: {message}"),
        }
    }
}
pub struct Session {
    handle: Option<OwnedHandle>,
    next_id: u64,
}

impl Session {
    /// Open one composition session. The caller keeps this value for the
    /// activation; dropping it closes the pipe and ends the service-side session.
    pub fn connect(name: &str) -> Result<Self, ClientError> {
        Ok(Self {
            handle: Some(open_pipe(name)?),
            next_id: 1,
        })
    }

    pub fn ping(&mut self) -> Result<(), ClientError> {
        let id = self.allocate_id();
        match self.roundtrip(ClientRequest::Ping { id })? {
            ClientResponse::Pong { .. } => Ok(()),
            ClientResponse::Error { message, .. } => Err(ClientError::Rejected(message)),
            ClientResponse::Update(_) => Err(ClientError::Protocol(ProtocolError::Invalid)),
        }
    }

    pub fn deactivate(&mut self) -> Result<(), ClientError> {
        let id = self.allocate_id();
        match self.roundtrip(ClientRequest::Deactivate { id })? {
            ClientResponse::Update(_) | ClientResponse::Pong { .. } => Ok(()),
            ClientResponse::Error { message, .. } => Err(ClientError::Rejected(message)),
        }
    }

    pub fn key(&mut self, key: KeyRequest) -> Result<CompositionUpdate, ClientError> {
        let id = self.allocate_id();
        match self.roundtrip(ClientRequest::Key { id, key })? {
            ClientResponse::Update(update) => Ok(update),
            ClientResponse::Error { message, .. } => Err(ClientError::Rejected(message)),
            ClientResponse::Pong { .. } => Err(ClientError::Protocol(ProtocolError::Invalid)),
        }
    }

    fn allocate_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        id
    }

    fn roundtrip(&mut self, request: ClientRequest) -> Result<ClientResponse, ClientError> {
        let Some(handle) = self.handle.as_ref() else {
            return Err(ClientError::Unavailable);
        };
        let frame = protocol::encode_request_frame(&request).map_err(ClientError::Protocol)?;
        if let Err(err) = write_all(handle, &frame) {
            self.handle = None;
            return Err(err);
        }
        let payload = match read_frame(handle) {
            Ok(payload) => payload,
            Err(err) => {
                self.handle = None;
                return Err(err);
            }
        };
        match protocol::parse_response(&payload) {
            Ok(response) => Ok(response),
            Err(err) => {
                self.handle = None;
                Err(ClientError::Protocol(err))
            }
        }
    }
}

fn open_pipe(name: &str) -> Result<OwnedHandle, ClientError> {
    match open_once(name) {
        Ok(handle) => Ok(handle),
        Err(ClientError::Unavailable) => {
            let wide = wide_null(name);
            // SAFETY: `wide` is NUL-terminated. A failed wait means the pipe is
            // not available; the caller returns the key instead of blocking.
            let ready = unsafe { WaitNamedPipeW(PCWSTR(wide.as_ptr()), IO_TIMEOUT_MS).as_bool() };
            if !ready {
                return Err(ClientError::Unavailable);
            }
            open_once(name)
        }
        Err(err) => Err(err),
    }
}

fn open_once(name: &str) -> Result<OwnedHandle, ClientError> {
    let wide = wide_null(name);
    // SAFETY: `wide` is NUL-terminated. The handle is owned by the returned
    // `OwnedHandle`. Overlapped mode is required so reads and writes can time out.
    let opened = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            (windows::Win32::Foundation::GENERIC_READ | windows::Win32::Foundation::GENERIC_WRITE)
                .0,
            FILE_SHARE_MODE(0),
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OVERLAPPED,
            None,
        )
    };
    match opened {
        Ok(handle) => Ok(unsafe { OwnedHandle::from_raw_handle(handle.0) }),
        Err(err) if is_unavailable(&err) => Err(ClientError::Unavailable),
        Err(err) => Err(windows_error(err)),
    }
}

fn read_frame(handle: &OwnedHandle) -> Result<Vec<u8>, ClientError> {
    let mut header = [0u8; 4];
    read_exact(handle, &mut header)?;
    let len = u32::from_le_bytes(header) as usize;
    if len == 0 {
        return Err(ClientError::Protocol(ProtocolError::Empty));
    }
    if len > protocol::MAX_FRAME_LEN {
        return Err(ClientError::Protocol(ProtocolError::TooLarge));
    }
    let mut payload = vec![0u8; len];
    read_exact(handle, &mut payload)?;
    Ok(payload)
}

fn read_exact(handle: &OwnedHandle, buf: &mut [u8]) -> Result<(), ClientError> {
    let mut filled = 0;
    while filled < buf.len() {
        let n = read_some(handle, &mut buf[filled..])?;
        if n == 0 {
            return Err(ClientError::Disconnected);
        }
        filled += n;
    }
    Ok(())
}

fn write_all(handle: &OwnedHandle, mut data: &[u8]) -> Result<(), ClientError> {
    while !data.is_empty() {
        let n = write_some(handle, data)?;
        if n == 0 {
            return Err(ClientError::Disconnected);
        }
        data = &data[n..];
    }
    Ok(())
}

fn read_some(handle: &OwnedHandle, buf: &mut [u8]) -> Result<usize, ClientError> {
    let (event, mut overlapped) = overlapped_event()?;
    // SAFETY: `buf` and `overlapped` stay alive until `finish_io` returns,
    // including the cancel wait after a timeout.
    let started = unsafe { ReadFile(raw(handle), Some(buf), None, Some(&mut overlapped)) };
    finish_io(handle, &event, &overlapped, started)
}

fn write_some(handle: &OwnedHandle, data: &[u8]) -> Result<usize, ClientError> {
    let (event, mut overlapped) = overlapped_event()?;
    // SAFETY: `data` and `overlapped` stay alive until `finish_io` returns.
    let started = unsafe { WriteFile(raw(handle), Some(data), None, Some(&mut overlapped)) };
    finish_io(handle, &event, &overlapped, started)
}

fn overlapped_event() -> Result<(OwnedHandle, OVERLAPPED), ClientError> {
    // SAFETY: an unnamed manual-reset event. OwnedHandle closes it.
    let event = unsafe { CreateEventW(None, true, false, PCWSTR::null()).map_err(windows_error)? };
    let event = unsafe { OwnedHandle::from_raw_handle(event.0) };
    let overlapped = OVERLAPPED {
        hEvent: HANDLE(event.as_raw_handle()),
        ..OVERLAPPED::default()
    };
    Ok((event, overlapped))
}

fn finish_io(
    handle: &OwnedHandle,
    event: &OwnedHandle,
    overlapped: &OVERLAPPED,
    started: Result<(), Error>,
) -> Result<usize, ClientError> {
    match started {
        Ok(()) => transferred(handle, overlapped),
        Err(err) if is_win32(&err, ERROR_IO_PENDING.0) => {
            let wait = unsafe { WaitForSingleObject(raw_owned(event), IO_TIMEOUT_MS) };
            if wait == WAIT_TIMEOUT {
                // SAFETY: cancel the pending operation on this handle, then wait
                // until the kernel releases `overlapped` before returning.
                unsafe {
                    let _ = CancelIoEx(raw(handle), Some(overlapped));
                    let _ = WaitForSingleObject(raw_owned(event), 5_000);
                }
                return Err(ClientError::Timeout);
            }
            if wait != WAIT_OBJECT_0 {
                return Err(ClientError::Windows(format!("wait failed: {}", wait.0)));
            }
            transferred(handle, overlapped)
        }
        Err(err) if is_disconnect(&err) => Err(ClientError::Disconnected),
        Err(err) => Err(windows_error(err)),
    }
}

fn transferred(handle: &OwnedHandle, overlapped: &OVERLAPPED) -> Result<usize, ClientError> {
    let mut count = 0u32;
    // SAFETY: `overlapped` is the operation started on `handle`, and it has completed.
    match unsafe { GetOverlappedResult(raw(handle), overlapped, &mut count, false) } {
        Ok(()) => Ok(count as usize),
        Err(err) if is_disconnect(&err) => Err(ClientError::Disconnected),
        Err(err) if is_win32(&err, 995) => Err(ClientError::Timeout),
        Err(err) => Err(windows_error(err)),
    }
}

fn raw(handle: &OwnedHandle) -> HANDLE {
    HANDLE(handle.as_raw_handle())
}

fn raw_owned(handle: &OwnedHandle) -> HANDLE {
    raw(handle)
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn windows_error(err: Error) -> ClientError {
    ClientError::Windows(err.to_string())
}

fn is_unavailable(err: &Error) -> bool {
    is_win32(err, ERROR_FILE_NOT_FOUND.0)
        || is_win32(err, ERROR_PIPE_BUSY.0)
        || is_win32(err, ERROR_ACCESS_DENIED.0)
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
    use crate::protocol::{KeyKind, PIPE_NAME};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};
    use windows::Win32::{
        Foundation::ERROR_PIPE_CONNECTED,
        Storage::FileSystem::PIPE_ACCESS_DUPLEX,
        System::Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE,
            PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
        },
    };

    #[test]
    fn io_timeout_is_at_most_100ms() {
        assert_eq!(PIPE_NAME, r"\\.\pipe\Graver");
    }

    #[test]
    fn missing_pipe_returns_without_hanging() {
        let name = unique_pipe_name("Missing");
        assert_ne!(name, PIPE_NAME);
        let start = Instant::now();
        match Session::connect(&name) {
            Err(ClientError::Unavailable) => {}
            Err(err) => panic!("expected unavailable, got {err:?}"),
            Ok(_) => panic!("missing pipe connected"),
        }
        assert!(start.elapsed() < Duration::from_millis(800));
    }

    #[test]
    fn read_times_out_when_the_service_does_not_answer() {
        let name = unique_pipe_name("Timeout");
        let ready = AtomicBool::new(false);
        thread::scope(|scope| {
            let server = scope.spawn(|| stub_server(&name, &ready, None));
            wait_ready(&server, &ready, &name);
            let start = Instant::now();
            let mut session = Session::connect(&name).unwrap();
            let err = session
                .key(KeyRequest {
                    kind: KeyKind::Char('a'),
                    shift: false,
                    ctrl: false,
                    alt: false,
                })
                .unwrap_err();
            let elapsed = start.elapsed();
            drop(session);
            server.join().unwrap();
            assert!(matches!(err, ClientError::Timeout), "{err:?}");
            assert!(elapsed < Duration::from_millis(1_000), "{elapsed:?}");
        });
    }

    #[test]
    fn ping_roundtrip_uses_the_handwritten_frame() {
        let name = unique_pipe_name("Ping");
        let ready = AtomicBool::new(false);
        let reply = protocol::encode_frame(br#"{"op":"pong","v":1,"id":1}"#).unwrap();
        thread::scope(|scope| {
            let server = scope.spawn(|| stub_server(&name, &ready, Some(reply)));
            wait_ready(&server, &ready, &name);
            let mut session = Session::connect(&name).unwrap();
            session.ping().unwrap();
            drop(session);
            server.join().unwrap();
        });
    }

    fn unique_pipe_name(label: &str) -> String {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        format!(r"\\.\pipe\Graver.Tsf.{label}.{}.{n}", std::process::id())
    }

    fn wait_ready(server: &thread::ScopedJoinHandle<()>, ready: &AtomicBool, name: &str) {
        let start = Instant::now();
        while !ready.load(Ordering::SeqCst) {
            if server.is_finished() {
                panic!("stub server exited before listening");
            }
            if start.elapsed() > Duration::from_secs(5) {
                panic!("timed out waiting for {name}");
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn stub_server(name: &str, ready: &AtomicBool, reply: Option<Vec<u8>>) {
        let wide = wide_null(name);
        let handle = unsafe {
            CreateNamedPipeW(
                PCWSTR(wide.as_ptr()),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                PIPE_UNLIMITED_INSTANCES,
                64 * 1024,
                64 * 1024,
                0,
                None,
            )
        };
        assert!(!handle.is_invalid(), "stub pipe create failed");
        let pipe = unsafe { OwnedHandle::from_raw_handle(handle.0) };
        ready.store(true, Ordering::SeqCst);
        match unsafe { ConnectNamedPipe(raw(&pipe), None) } {
            Ok(()) => {}
            Err(err) if is_win32(&err, ERROR_PIPE_CONNECTED.0) => {}
            Err(err) => panic!("stub connect failed: {err}"),
        }
        let mut buf = [0u8; 512];
        let _ = blocking_read(&pipe, &mut buf);
        if let Some(reply) = reply {
            blocking_write(&pipe, &reply);
        }
        let _ = blocking_read(&pipe, &mut buf);
    }

    fn blocking_read(pipe: &OwnedHandle, buf: &mut [u8]) -> Result<usize, ClientError> {
        let mut read = 0u32;
        match unsafe { ReadFile(raw(pipe), Some(buf), Some(&mut read), None) } {
            Ok(()) => Ok(read as usize),
            Err(err) => Err(windows_error(err)),
        }
    }

    fn blocking_write(pipe: &OwnedHandle, data: &[u8]) {
        let mut written = 0u32;
        unsafe { WriteFile(raw(pipe), Some(data), Some(&mut written), None) }.expect("stub write");
    }
}
