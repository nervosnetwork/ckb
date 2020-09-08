use std::{marker::Unpin, net::SocketAddr, str::FromStr, sync::Arc};

use tokio::{
    io::{AsyncWriteExt, BufReader},
    net::TcpStream,
    prelude::*,
    sync::RwLock,
};

use ckb_logger::{debug, error, info, trace, warn};

use crate::{
    commands,
    common::{Arguments, Error},
};

async fn console_reply<W>(writer: &mut W, client: &SocketAddr, data: &str) -> Result<(), ()>
where
    W: AsyncWriteExt + Unpin,
{
    writer
        .write_all(data.as_bytes())
        .await
        .map_err(|error| {
            error!(
                "failed to send data to the client {}, since {}",
                client, error
            );
        })
        .map(|_| ())
}

async fn console_prompt<W>(writer: &mut W, client: &SocketAddr) -> Result<(), ()>
where
    W: AsyncWriteExt + Unpin,
{
    console_reply(writer, client, "> ").await
}

async fn console_error<W>(writer: &mut W, client: &SocketAddr, error: &Error) -> Result<(), ()>
where
    W: AsyncWriteExt + Unpin,
{
    let message = format!("Error {}\n", error);
    console_reply(writer, client, &message).await
}

pub(crate) async fn reject(mut stream: TcpStream, addr: SocketAddr) {
    let message = "Error only one client is allowed at one time.\n";
    let _ret = console_reply(&mut stream, &addr, message).await;
    debug!("client {} has been rejected", addr);
}

pub(crate) async fn serve(mut stream: TcpStream, addr: SocketAddr, has_client: Arc<RwLock<bool>>) {
    info!("serving the client {} ...", addr);
    let (read_half, mut writer) = stream.split();
    let mut reader = BufReader::new(read_half);
    let mut has_prompt = true;
    'stream_close: loop {
        if has_prompt && console_prompt(&mut writer, &addr).await.is_err() {
            break 'stream_close;
        }
        let mut input = String::new();
        if let Ok(num_bytes) = reader.read_line(&mut input).await {
            if num_bytes == 0 {
                debug!("client {} send EOF", addr);
                break 'stream_close;
            }
            match Arguments::from_str(&input) {
                Ok(args) => {
                    trace!("client {} send {}", addr, args);
                    match commands::execute(args).await {
                        Ok(ret) => {
                            if let Some(output) = ret.output() {
                                if console_reply(&mut writer, &addr, &output).await.is_err() {
                                    break 'stream_close;
                                }
                            }
                            if ret.disconnect() {
                                break 'stream_close;
                            }
                            if let Some(new_has_prompt) = ret.has_prompt() {
                                has_prompt = new_has_prompt;
                            }
                        }
                        Err(exec_error) => {
                            if console_error(&mut writer, &addr, &exec_error)
                                .await
                                .is_err()
                            {
                                break 'stream_close;
                            }
                        }
                    }
                }
                Err(args_error) => {
                    trace!("client {} send {:?}", addr, input);
                    if console_error(&mut writer, &addr, &args_error)
                        .await
                        .is_err()
                    {
                        break 'stream_close;
                    }
                }
            }
        } else {
            warn!("client {} send control characters", addr);
            break 'stream_close;
        }
    }
    *has_client.write().await = false;
}
