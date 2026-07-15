use std::time::Duration;
use std::time::SystemTime;

use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tracing::debug;

use crate::channel::ProxyToUi;
use crate::http::models::{RequestId, WsDirection, WsMessage};

type ParsedFrame = (Bytes, u8, Vec<u8>);

pub async fn relay<C, S>(
    mut client: C,
    mut server: S,
    request_id: RequestId,
    ui_tx: mpsc::Sender<ProxyToUi>,
    in_scope: bool,
    max_frame_bytes: usize,
) where
    C: AsyncRead + AsyncWrite + Unpin,
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut client_buf = BytesMut::with_capacity(8192);
    let mut server_buf = BytesMut::with_capacity(8192);
    let mut client_tmp = [0u8; 8192];
    let mut server_tmp = [0u8; 8192];
    let idle = tokio::time::sleep(Duration::from_secs(120));
    tokio::pin!(idle);

    loop {
        tokio::select! {
            _ = &mut idle => break,
            result = client.read(&mut client_tmp) => {
                match result {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        idle.as_mut().reset(tokio::time::Instant::now() + Duration::from_secs(120));
                        client_buf.extend_from_slice(&client_tmp[..n]);
                        loop {
                            let (raw, opcode, payload) = match parse_next_frame(&mut client_buf, max_frame_bytes) {
                                ParseNext::Frame(frame) => frame,
                                ParseNext::Incomplete => break,
                                ParseNext::Invalid => return,
                            };
                            if in_scope && is_data_frame(opcode) {
                                let msg = WsMessage {
                                    direction: WsDirection::ClientToServer,
                                    opcode,
                                    payload: Bytes::from(payload),
                                    timestamp: SystemTime::now(),
                                };
                                let _ = ui_tx.try_send(ProxyToUi::WebSocketFrame(request_id, msg));
                            }
                            if server.write_all(&raw).await.is_err() {
                                return;
                            }
                        }
                    }
                }
            }
            result = server.read(&mut server_tmp) => {
                match result {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        idle.as_mut().reset(tokio::time::Instant::now() + Duration::from_secs(120));
                        server_buf.extend_from_slice(&server_tmp[..n]);
                        loop {
                            let (raw, opcode, payload) = match parse_next_frame(&mut server_buf, max_frame_bytes) {
                                ParseNext::Frame(frame) => frame,
                                ParseNext::Incomplete => break,
                                ParseNext::Invalid => return,
                            };
                            if in_scope && is_data_frame(opcode) {
                                let msg = WsMessage {
                                    direction: WsDirection::ServerToClient,
                                    opcode,
                                    payload: Bytes::from(payload),
                                    timestamp: SystemTime::now(),
                                };
                                let _ = ui_tx.try_send(ProxyToUi::WebSocketFrame(request_id, msg));
                            }
                            if client.write_all(&raw).await.is_err() {
                                return;
                            }
                        }
                    }
                }
            }
        }
    }

    debug!("WebSocket relay for request {} finished", request_id);
}

enum ParseNext {
    Incomplete,
    Frame(ParsedFrame),
    Invalid,
}

fn parse_next_frame(buf: &mut BytesMut, max_frame_bytes: usize) -> ParseNext {
    match try_parse_frame(buf, max_frame_bytes) {
        Ok(Some(frame)) => ParseNext::Frame(frame),
        Ok(None) => ParseNext::Incomplete,
        Err(error) => {
            debug!("Closing WebSocket relay: {error}");
            buf.clear();
            ParseNext::Invalid
        }
    }
}

fn is_data_frame(opcode: u8) -> bool {
    opcode == 1 || opcode == 2
}

fn try_parse_frame(
    buf: &mut BytesMut,
    max_frame_bytes: usize,
) -> Result<Option<ParsedFrame>, &'static str> {
    if buf.len() < 2 {
        return Ok(None);
    }

    let b0 = buf[0];
    let b1 = buf[1];
    let opcode = b0 & 0x0F;
    let masked = (b1 & 0x80) != 0;
    let mut payload_len = (b1 & 0x7F) as u64;

    let mut offset = 2usize;

    if payload_len == 126 {
        if buf.len() < 4 {
            return Ok(None);
        }
        payload_len = u16::from_be_bytes([buf[2], buf[3]]) as u64;
        offset = 4;
    } else if payload_len == 127 {
        if buf.len() < 10 {
            return Ok(None);
        }
        payload_len = u64::from_be_bytes([
            buf[2], buf[3], buf[4], buf[5], buf[6], buf[7], buf[8], buf[9],
        ]);
        offset = 10;
    }

    let payload_len =
        usize::try_from(payload_len).map_err(|_| "frame length does not fit usize")?;
    if payload_len > max_frame_bytes {
        return Err("frame exceeds configured limit");
    }
    let mask_size = if masked { 4usize } else { 0 };
    let total = offset
        .checked_add(mask_size)
        .and_then(|value| value.checked_add(payload_len))
        .ok_or("frame length overflow")?;

    if buf.len() < total {
        return Ok(None);
    }

    let raw = buf.split_to(total);

    let mask_key = if masked {
        Some([
            raw[offset],
            raw[offset + 1],
            raw[offset + 2],
            raw[offset + 3],
        ])
    } else {
        None
    };

    let payload_start = offset + mask_size;
    let mut payload = raw[payload_start..].to_vec();

    if let Some(mask) = mask_key {
        for (i, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[i % 4];
        }
    }

    Ok(Some((raw.freeze(), opcode, payload)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_frames_above_limit() {
        let mut frame = BytesMut::from(&b"\x82\x7e\x00\x20"[..]);
        assert_eq!(
            try_parse_frame(&mut frame, 16),
            Err("frame exceeds configured limit")
        );
    }

    #[test]
    fn rejects_overflowing_64_bit_length() {
        let mut frame = BytesMut::from(&b"\x82\x7f\xff\xff\xff\xff\xff\xff\xff\xff"[..]);
        assert!(try_parse_frame(&mut frame, usize::MAX).is_err());
    }

    #[test]
    fn parses_small_frame() {
        let mut frame = BytesMut::from(&b"\x81\x02ok"[..]);
        let (_, opcode, payload) = try_parse_frame(&mut frame, 16).unwrap().unwrap();
        assert_eq!(opcode, 1);
        assert_eq!(payload, b"ok");
    }
}
