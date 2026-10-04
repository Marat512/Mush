use std::io;
use std::marker::Unpin;

use serde::{de::DeserializeOwned, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const TYPE_TEXT: u8 = 0x01;
pub const TYPE_SYNC: u8 = 0x02;
pub const TYPE_HANDSHAKE: u8 = 0x03;
pub const TYPE_DECOY: u8 = 0x04;

const MAX_FRAME: u32 = 16 * 1024 * 1024;

/// One framed message: [4 bytes BE length][1 byte type][CBOR payload],
/// where `length` covers type + payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub msg_type: u8,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn new(msg_type: u8, payload: Vec<u8>) -> Self {
        Frame { msg_type, payload }
    }

    /// Serialize a CBOR payload into a frame of the given type.
    pub fn typed<T: Serialize>(msg_type: u8, value: &T) -> io::Result<Frame> {
        let mut payload = Vec::new();
        ciborium::ser::into_writer(value, &mut payload)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(Frame::new(msg_type, payload))
    }

    /// Decode this frame's CBOR payload.
    pub fn decode<T: DeserializeOwned>(&self) -> io::Result<T> {
        ciborium::de::from_reader(&self.payload[..])
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

pub async fn write_frame<W: AsyncWriteExt + Unpin>(w: &mut W, frame: &Frame) -> io::Result<()> {
    let body_len = 1 + frame.payload.len();
    if body_len as u32 > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    w.write_u32(body_len as u32).await?;
    w.write_u8(frame.msg_type).await?;
    w.write_all(&frame.payload).await?;
    w.flush().await
}

/// Returns Ok(None) on clean EOF at a frame boundary.
pub async fn read_frame<R: AsyncReadExt + Unpin>(r: &mut R) -> io::Result<Option<Frame>> {
    let len = match r.read_u32().await {
        Ok(len) => len,
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    };
    if len == 0 || len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bad frame length",
        ));
    }
    let msg_type = r.read_u8().await?;
    let mut payload = vec![0u8; (len - 1) as usize];
    r.read_exact(&mut payload).await?;
    Ok(Some(Frame::new(msg_type, payload)))
}

#[allow(clippy::items_after_test_module)]
#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use tokio::io::duplex;
    use tokio::net::{TcpListener, TcpStream};

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Ping {
        seq: u64,
        text: String,
    }

    #[tokio::test]
    async fn frame_roundtrip_over_memory_pipe() {
        let (mut a, mut b) = duplex(1024);
        let frame = Frame::typed(
            TYPE_TEXT,
            &Ping {
                seq: 7,
                text: "hello".into(),
            },
        )
        .unwrap();
        write_frame(&mut a, &frame).await.unwrap();
        drop(a);
        let got = read_frame(&mut b).await.unwrap().expect("frame");
        assert_eq!(got.msg_type, TYPE_TEXT);
        let decoded: Ping = got.decode().unwrap();
        assert_eq!(
            decoded,
            Ping {
                seq: 7,
                text: "hello".into()
            }
        );
        assert!(read_frame(&mut b).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn ping_pong_over_tcp() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            while let Some(frame) = read_frame(&mut socket).await.unwrap() {
                assert_eq!(frame.msg_type, TYPE_TEXT);
                let ping: Ping = frame.decode().unwrap();
                let pong = Frame::typed(TYPE_TEXT, &ping).unwrap();
                write_frame(&mut socket, &pong).await.unwrap();
            }
        });

        let mut client = TcpStream::connect(addr).await.unwrap();
        for seq in 0..5 {
            let ping = Frame::typed(
                TYPE_TEXT,
                &Ping {
                    seq,
                    text: "mush".into(),
                },
            )
            .unwrap();
            write_frame(&mut client, &ping).await.unwrap();
            let reply = read_frame(&mut client).await.unwrap().expect("reply");
            let decoded: Ping = reply.decode().unwrap();
            assert_eq!(decoded.seq, seq);
        }
        drop(client);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn corrupt_length_rejected() {
        let (mut a, mut b) = duplex(1024);
        a.write_u32(0).await.unwrap(); // len 0 is invalid
        a.flush().await.unwrap();
        drop(a);
        assert!(read_frame(&mut b).await.is_err());
    }
}

// Этап A (ratchet-режим): 0x05 — FullManifest (DTN broadcast), 0x06 —
// X3dhInit (первый фрейм), 0x07 — RatchetFrame (сообщение сессии).
pub const TYPE_MANIFEST: u8 = 0x05;
pub const TYPE_X3DH_INIT: u8 = 0x06;
pub const TYPE_RATCHET: u8 = 0x07;
