use hyper::{
    server::Server,
    service::{make_service_fn, service_fn},
    Response, StatusCode,
};
use listenfd::ListenFd;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use tokio;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::signal;
use tracing::*;

use std::convert::Infallible;

use futures::stream::FuturesUnordered;
use futures::stream::StreamExt;

use tokio::runtime::Runtime;

use std::io::{self, prelude::*};
use std::net::*;

mod varint;

#[derive(Deserialize, Debug)]
struct ServerListResponse<'a> {
    #[serde(borrow)]
    description: Description<'a>,
    #[serde(borrow)]
    modinfo: ModInfo<'a>,
    #[serde(borrow)]
    players: Players<'a>,
}

#[derive(Deserialize, Debug)]
struct Description<'a> {
    text: &'a str,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct ModInfo<'a> {
    #[serde(borrow)]
    mod_list: Vec<Mod<'a>>,
}

#[derive(Deserialize, Debug)]
struct Mod<'a> {
    modid: &'a str,
    version: &'a str,
}

#[derive(Deserialize, Debug)]
struct Players<'a> {
    max: u64,
    online: u64,
    #[serde(borrow)]
    #[serde(default)]
    sample: Vec<Player<'a>>,
}

#[derive(Deserialize, Debug)]
struct Player<'a> {
    id: &'a str,
    name: &'a str,
}

#[derive(Debug, Serialize)]
enum ScanError {
    ConnectError(String),
    WriteError,
    ReadError,
    PacketFormatError,
    PleaseNoDoSError,
    JsonReadError,
    JsonParseError(String),
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    main2().await
}

async fn main2() {
    let make_svc = make_service_fn(|_conn| async {
        Ok::<_, Infallible>(service_fn(move |req| async move {
            let mut params: HashMap<String, String> = req
                .uri()
                .query()
                .map(|v| {
                    url::form_urlencoded::parse(v.as_bytes())
                        .into_owned()
                        .collect()
                })
                .unwrap_or_else(HashMap::new);

            let res: Result<Response<hyper::Body>, _> =
            if let Some(addr) = params.remove("target") {
                match scan_server(addr.clone()).await {
                    Ok(result) => {
                        Response::builder()
                            .body(result.into())
                    }, Err(err) => {
                        warn!(?addr, ?err, "scan-failed");

                        Response::builder()
                            .status(StatusCode::INTERNAL_SERVER_ERROR)
                            .body("".into())
                    }
                }
            } else {
                warn!("no-query-target");
                Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body("".into())
            };
            res
        }))
    });

    Server::from_tcp(
        ListenFd::from_env()
            .take_tcp_listener(0_usize)
            .expect("can't listenfd")
            .expect("no listenfd 0"),
    )
    .expect("can't make server")
    .serve(make_svc)
    .with_graceful_shutdown(shutdown_signal())
    .await
    .expect("can't serve");
}

async fn shutdown_signal() {
    signal::unix::signal(signal::unix::SignalKind::interrupt())
        .unwrap()
        .recv()
        .await
        .expect("failed to install SIGINT handler");
}

async fn scan_server(addr: String) -> Result<String, ScanError> {
    let buf = do_scan(addr.parse().unwrap()).await?;

    let parsed: ServerListResponse =
        serde_json::from_slice(&buf).map_err(|e| ScanError::JsonParseError(e.to_string()))?;

    Ok(format!(
        r#"
# TYPE minecraft_players_current gauge
minecraft_players_current {}

# TYPE minecraft_players_max gauge
minecraft_players_max {}

# TYPE minecraft_mod_count gauge
minecraft_mod_count {}
"#,
        parsed.players.online,
        parsed.players.max,
        parsed.modinfo.mod_list.len(),
    ))
}

async fn do_scan<'a>(addr: SocketAddr) -> Result<Vec<u8>, ScanError> {
    use ScanError::*;

    let mut stream = TcpStream::connect(&addr)
        .await
        .map_err(|e| ConnectError(e.to_string()))?;

    let pkts = [
        0x15, 0x00, 0xd4, 0x02, 0x0e, 0x6c, 0x6f, 0x63, 0x61, 0x6c, 0x68, 0x6f, 0x73, 0x74, 0x00,
        0x46, 0x4d, 0x4c, 0x00, 0x63, 0xdd, 0x01, // Second packet
        0x01, 0x00,
    ];
    stream.write(&pkts).await.map_err(|_| WriteError)?;

    varint::read_var_i64(&mut stream)
        .await
        .map_err(|_| PacketFormatError)?;
    let mut pkt_type: [u8; 1] = [0xFF];
    stream
        .read_exact(&mut pkt_type)
        .await
        .map_err(|_| ReadError)?;

    if pkt_type[0] != 0x00 {
        Err(PacketFormatError)?
    }

    let len = varint::read_var_i64(&mut stream)
        .await
        .map_err(|_| PacketFormatError)?;

    if len > (64 * 1024) {
        Err(PleaseNoDoSError)?
    }

    let mut jsonbuf = Vec::new();
    jsonbuf.resize(len as usize, 0x00);
    stream
        .read_exact(&mut jsonbuf)
        .await
        .map_err(|_| JsonReadError)?;

    Ok(jsonbuf)
}
