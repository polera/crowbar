use std::time::SystemTime;

use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tracing::debug;

use crate::channel::ProxyToUi;
use crate::http::models::{RequestId, WsDirection, WsMessage};

pub async fn relay<C, S>(
    mut client: C,
    mut server: S,
    request_id: RequestId,
    ui_tx: mpsc::UnboundedSender<ProxyToUi>,
    in_scope: bool,
) where
    C: AsyncRead + AsyncWrite + Unpin,
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut client_buf = BytesMut::with_capacity(8192);
    let mut server_buf = BytesMut::with_capacity(8192);
    let mut client_tmp = [0u8; 8192];
    let mut server_tmp = [0u8; 8192];

    loop {
        tokio::select! {
            result = client.read(&mut client_tmp) => {
                match result {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        client_buf.extend_from_slice(&client_tmp[..n]);
                        while let Some((raw, opcode, payload)) = try_parse_frame(&mut client_buf) {
                            if in_scope && is_data_frame(opcode) {
                                let msg = WsMessage {
                                    direction: WsDirection::ClientToServer,
                                    opcode,
                                    payload: Bytes::from(payload),
                                    timestamp: SystemTime::now(),
                                };
                                let _ = ui_tx.send(ProxyToUi::WebSocketFrame(request_id, msg));
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
                        server_buf.extend_from_slice(&server_tmp[..n]);
                        while let Some((raw, opcode, payload)) = try_parse_frame(&mut server_buf) {
                            if in_scope && is_data_frame(opcode) {
                                let msg = WsMessage {
                                    direction: WsDirection::ServerToClient,
                                    opcode,
                                    payload: Bytes::from(payload),
                                    timestamp: SystemTime::now(),
                                };
                                let _ = ui_tx.send(ProxyToUi::WebSocketFrame(request_id, msg));
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

fn is_data_frame(opcode: u8) -> bool {
    opcode == 1 || opcode == 2
}

fn try_parse_frame(buf: &mut BytesMut) -> Option<(Bytes, u8, Vec<u8>)> {
    if buf.len() < 2 {
        return None;
    }

    let b0 = buf[0];
    let b1 = buf[1];
    let opcode = b0 & 0x0F;
    let masked = (b1 & 0x80) != 0;
    let mut payload_len = (b1 & 0x7F) as u64;

    let mut offset = 2usize;

    if payload_len == 126 {
        if buf.len() < 4 {
            return None;
        }
        payload_len = u16::from_be_bytes([buf[2], buf[3]]) as u64;
        offset = 4;
    } else if payload_len == 127 {
        if buf.len() < 10 {
            return None;
        }
        payload_len = u64::from_be_bytes([
            buf[2], buf[3], buf[4], buf[5], buf[6], buf[7], buf[8], buf[9],
        ]);
        offset = 10;
    }

    let mask_size = if masked { 4 } else { 0 };
    let total = offset + mask_size + payload_len as usize;

    if buf.len() < total {
        return None;
    }

    let raw = buf.split_to(total);

    let mask_key = if masked {
        Some([raw[offset], raw[offset + 1], raw[offset + 2], raw[offset + 3]])
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

    Some((raw.freeze(), opcode, payload))
}
