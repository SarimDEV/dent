//! A TCP proxy between client and daemon for testing: it can record every
//! byte that crosses it (to check nothing readable does) and cut every
//! connection after a while (a flaky network).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

pub struct Proxy {
    pub addr: SocketAddr,
    pub tap: Arc<Mutex<Vec<u8>>>,
    pub cuts: Arc<Mutex<u64>>,
}

pub async fn start(target: SocketAddr, cut_after: Option<Duration>) -> Result<Proxy> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let tap = Arc::new(Mutex::new(Vec::new()));
    let cuts = Arc::new(Mutex::new(0));
    let (t, c) = (tap.clone(), cuts.clone());
    tokio::spawn(async move {
        while let Ok((inbound, _)) = listener.accept().await {
            let (t, c) = (t.clone(), c.clone());
            tokio::spawn(async move {
                let Ok(outbound) = TcpStream::connect(target).await else {
                    return;
                };
                let (mut ir, mut iw) = inbound.into_split();
                let (mut or, mut ow) = outbound.into_split();
                let up = pipe(&mut ir, &mut ow, t.clone());
                let down = pipe(&mut or, &mut iw, t);
                match cut_after {
                    Some(d) => {
                        tokio::select! {
                            _ = async { tokio::join!(up, down) } => {}
                            _ = tokio::time::sleep(d) => { *c.lock().await += 1; }
                        }
                    }
                    None => {
                        let _ = tokio::join!(up, down);
                    }
                }
            });
        }
    });
    Ok(Proxy { addr, tap, cuts })
}

async fn pipe(
    from: &mut tokio::net::tcp::OwnedReadHalf,
    to: &mut tokio::net::tcp::OwnedWriteHalf,
    tap: Arc<Mutex<Vec<u8>>>,
) {
    let mut buf = vec![0u8; 16384];
    loop {
        let n = match from.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        {
            let mut t = tap.lock().await;
            if t.len() < 32 << 20 {
                t.extend_from_slice(&buf[..n]);
            }
        }
        if to.write_all(&buf[..n]).await.is_err() {
            break;
        }
    }
    let _ = to.shutdown().await;
}
