use std::io::{Read, Write};

use graver_engine::Engine;
use graver_ipc::{FrameDecoder, ProtocolError, decode_request, encode_response};

use crate::{ServiceError, dispatch};

pub fn serve_stream<R: Read, W: Write>(
    engine: &mut Engine,
    mut reader: R,
    mut writer: W,
) -> Result<(), ServiceError> {
    let mut decoder = FrameDecoder::new();
    let mut buf = [0u8; 4096];
    loop {
        let read = reader.read(&mut buf)?;
        if read == 0 {
            if decoder.pending() > 0 {
                return Err(ProtocolError::UnexpectedEof.into());
            }
            return Ok(());
        }
        for payload in decoder.push(&buf[..read])? {
            let request = decode_request(&payload)?;
            let response = dispatch(engine, request);
            writer.write_all(&encode_response(&response)?)?;
            writer.flush()?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graver_ipc::{Request, Response, decode_response, encode_request};
    use std::io::{Read, Write};
    use std::thread;

    #[test]
    fn stdio_frames_reach_the_engine() {
        let (server_read, mut client_write) = std::io::pipe().unwrap();
        let (mut client_read, server_write) = std::io::pipe().unwrap();
        let server = thread::spawn(move || {
            let mut engine = Engine::new();
            serve_stream(&mut engine, server_read, server_write)
        });

        let request = encode_request(&Request::Ping { v: 1, id: 8 }).unwrap();
        client_write.write_all(&request).unwrap();
        drop(client_write);

        let mut incoming = Vec::new();
        client_read.read_to_end(&mut incoming).unwrap();
        server.join().unwrap().unwrap();

        let mut decoder = FrameDecoder::new();
        let frames = decoder.push(&incoming).unwrap();
        assert_eq!(frames.len(), 1);
        assert!(matches!(
            decode_response(&frames[0]).unwrap(),
            Response::Pong { id: 8, .. }
        ));
    }
}
