use crate::common::{CubIpRelayAction, CubIpResponse};
use crate::config::Config;
use crate::Task;
use anyhow::{anyhow, Context};
use log::{info, warn};
use paho_mqtt as mqtt;
use quick_xml::de::from_str;
use std::sync::Arc;
use tokio::io;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::mpsc::UnboundedSender;

pub async fn request_to_cub_ip(config: &Arc<Config>, path: &str) -> anyhow::Result<Vec<u8>> {
    let authorization_base64 = base64::encode(format!(
        "{}:{}",
        config.cub_ip_login, config.cub_ip_password
    ));

    let raw_request = format!(
        "GET {} HTTP/1.1\r\n\
Host: {}\r\n\
User-Agent: cub-ip-app\r\n\
Accept: */*\r\n\
Authorization: Basic {}\r\n\
\r\n\
",
        path, &config.cub_ip_address, authorization_base64
    );

    let mut stream = tokio::time::timeout(
        config.cub_ip_connection_timeout,
        TcpStream::connect(&config.cub_ip_address),
    )
    .await
    .with_context(|| "connection timeout expired.")??;

    // Write some data.
    tokio::time::timeout(
        config.cub_ip_write_timeout,
        stream.write_all(raw_request.as_bytes()),
    )
    .await
    .with_context(|| "write timeout expired.")??;

    let mut response_buf = Vec::new();

    loop {
        // Wait for the socket to be readable
        tokio::time::timeout(config.cub_ip_read_timeout, stream.readable())
            .await
            .with_context(|| "read timeout expired.")??;

        // Creating the buffer **after** the `await` prevents it from
        // being stored in the async task.
        let mut buf = [0; 4096];

        // Try to read data, this may still fail with `WouldBlock`
        // if the readiness event is a false positive.
        match stream.try_read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                response_buf.extend_from_slice(&buf[..n]);
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                continue;
            }
            Err(e) => {
                return Err(e.into());
            }
        }
    }

    Ok(response_buf)
}

pub async fn request_relay_action(
    config: &Arc<Config>,
    action: CubIpRelayAction,
) -> anyhow::Result<()> {
    let parameters = match action {
        CubIpRelayAction::RelayTurnOn => (1usize, 0),
        CubIpRelayAction::RelayTurnOff => (0, 0),
        CubIpRelayAction::TimeRelayTurnOn(time) => (3, time),
        CubIpRelayAction::TimeRelayTurnOff(time) => (2, time),
    };
    let response_buf = request_to_cub_ip(
        config,
        format!("/sw.cgi?r=r{}&val={}", parameters.0, parameters.1).as_str(),
    )
    .await?;
    let is_response_ok = std::str::from_utf8(&response_buf)
        .ok()
        .and_then(|str| str.find("HTTP/1.1 200 OK"))
        .is_some();

    info!(
        "request relay action: {:?} is {}",
        parameters, is_response_ok
    );
    Ok(())
}

pub async fn request_status(
    config: Arc<Config>,
    sender: UnboundedSender<Task>,
    mqtt_client: mqtt::AsyncClient,
) {
    let request_state = || {
        async {
            let response_buf = request_to_cub_ip(&config, "/status.xml").await?;
            let raw_xml = std::str::from_utf8(&response_buf).ok().and_then(|str| {
                let xmls = str
                    .split("\r\n\r\n")
                    .filter(|x| x.starts_with("<?xml version="))
                    .collect::<Vec<&str>>();
                if xmls.len() == 1 {
                    return Some(xmls[0]);
                }
                None
            });

            if let Some(xml) = raw_xml {
                let response: CubIpResponse = from_str(xml)?;
                //println!("response: {:?}", response);
                return Ok::<CubIpResponse, anyhow::Error>(response);
            }

            Err(anyhow!("Response failed"))
        }
    };

    loop {
        if mqtt_client.is_connected() {
            if let Err(e) = sender.send(Task::Response(Box::new(request_state().await))) {
                warn!("send to mqtt worker failed ({:?})", e);
                break;
            }
        }
        tokio::time::sleep(config.cub_ip_request_timeout).await;
    }
}
