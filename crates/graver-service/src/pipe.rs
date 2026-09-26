//! Named-pipe transport.
//!
//! The default DACL still grants read access to Everyone. Tighten it to the
//! current user before this process listens for a real input session.

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

use crate::{ServiceError, dispatch};

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

fn create_pipe(name: &str) -> Result<OwnedHandle, ServiceError> {
    let wide = wide_null(name);
    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(wide.as_ptr()),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            64 * 1024,
            64 * 1024,
            0,
            None,
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
    use graver_ipc::{PIPE_NAME, Request, Response, decode_response, encode_request};
    use std::time::{Duration, Instant};
    use windows::Win32::{
        Foundation::{GENERIC_READ, GENERIC_WRITE},
        Storage::FileSystem::{CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_MODE, OPEN_EXISTING},
    };

    #[test]
    fn ping_over_named_pipe() {
        let name = format!("{PIPE_NAME}.Test.{}", std::process::id());
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

    fn client_exchange(name: &str, request: &Request) -> Result<Response, ServiceError> {
        let wide = wide_null(name);
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
        let pipe = unsafe { OwnedHandle::from_raw_handle(handle.0) };
        write_all(&pipe, &encode_request(request)?)?;
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
