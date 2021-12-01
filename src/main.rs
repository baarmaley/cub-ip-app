use anyhow::{anyhow, Context};
use log::warn;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
extern crate quick_xml;
extern crate serde;

use paho_mqtt as mqtt;
use paho_mqtt::QOS_1;
use quick_xml::de::from_str;
use serde::Deserialize;

#[derive(Debug, Deserialize, PartialEq)]
struct CubIpResponse {
    tm: String,
    stv: String,
    trm: String, // temperatura
    hih: String,
    vle0: String, // pin 1
    vle1: String, // pin 2
    vle2: String,
    vle3: String,
    vle4: String,
    vle5: String,
    strel: String, // relay 0: turn off; 1: turn on;  2: turn off (time relay) 3: turn on (time relay)
    tmrel: String, // time relay
}

struct Config {
    pub cub_ip_address: String,
    pub cub_ip_login: String,
    pub cub_ip_password: String,
    pub cub_ip_connection_timeout: Duration,
    pub cub_ip_read_timeout: Duration,
    pub cub_ip_write_timeout: Duration,
    pub cub_ip_request_timeout: Duration,
    pub mqtt_main_topic: String,
    pub mqtt_host: String,
    pub mqtt_name_client: String,
}

impl Config {
    pub fn read_from_file(path: &str) -> Config {
        let cub_ip_address = "192.168.0.117:80".to_string();
        let localhost = "127.0.0.1:8080";
        let cub_ip_login = "admin".to_string();
        let cub_ip_password = "5555".to_string();
        let cub_ip_connection_timeout = Duration::from_millis(500);
        let cub_ip_read_timeout = Duration::from_millis(500);
        let cub_ip_write_timeout = Duration::from_millis(500);
        let cub_ip_request_timeout = Duration::from_millis(500);
        let mqtt_main_topic = "cub_ip_device".to_string();
        let mqtt_host = "192.168.0.114:1883".to_string();
        let mqtt_name_client = "cub_ip_service".to_string();
        Config {
            cub_ip_address,
            cub_ip_login,
            cub_ip_password,
            cub_ip_connection_timeout,
            cub_ip_read_timeout,
            cub_ip_write_timeout,
            cub_ip_request_timeout,
            mqtt_main_topic,
            mqtt_host,
            mqtt_name_client,
        }
    }
}


async fn tcp_cub_ip_client(
    config: Arc<Config>,
    sender: UnboundedSender<Task>,
    mqtt_connected_flag: Arc<AtomicBool>,
) {
    let authorization_base64 = base64::encode(format!(
        "{}:{}",
        config.cub_ip_login, config.cub_ip_password
    ));

    let raw_request = format!(
        "GET /status.xml HTTP/1.1\r\n\
Host: {}\r\n\
User-Agent: cub-ip-app\r\n\
Accept: */*\r\n\
Authorization: Basic {}\r\n\
\r\n\
",
        &config.cub_ip_address, authorization_base64
    );

    let request_state = || {
        async {
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
        if mqtt_connected_flag.load(Ordering::SeqCst) {
            if let Err(e) = sender.send(Task::Response(request_state().await)) {
                warn!("send to mqtt worker failed ({:?})", e);
                break;
            }
        }
        tokio::time::sleep(config.cub_ip_request_timeout).await;
    }
}

#[derive(Debug)]
enum Task {
    Response(anyhow::Result<CubIpResponse>),
    ResetPrevValue,
}

async fn publish_version(version_topic: &str, mqtt_client: &mqtt::AsyncClient) {
    if let Err(e) = mqtt_client
        .publish(mqtt::Message::new_retained(
            version_topic,
            env!("CARGO_PKG_VERSION"),
            QOS_1,
        ))
        .await
    {
        warn!("Publish verstion failed ({:?})", e);
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let config = Arc::new(Config::read_from_file(""));

    let temperature_topic = format!("/{}/temperature", config.mqtt_main_topic);
    let relay_topic = format!("/{}/relay/state", config.mqtt_main_topic);
    let pin_1_topic = format!("/{}/pin_1/state", config.mqtt_main_topic);
    let pin_2_topic = format!("/{}/pin_2/state", config.mqtt_main_topic);
    let version_topic = format!("/{}/version", config.mqtt_main_topic);

    let decode_value_temperature_sensor = |v: &str| {
        if v == "Обрыв датчика" {
            return Ok("-".to_string());
        }
        if v.len() > 2 {
            return Ok(v[0..(v.len() - 3)].to_string());
        }
        Err(anyhow!("unknown temperature value (value: {}).", v))
    };

    let decode_value_relay_state = |v: &str| match v {
        "0" | "2" => Ok("0".to_string()),
        "1" | "3" => Ok("1".to_string()),
        _ => Err(anyhow!("unknown relay value (value: {}).", v)),
    };

    let decode_value_pin_state = |v: &str| match v {
        "Сработка" => Ok("0".to_string()),
        "Норма" => Ok("1".to_string()),
        _ => Err(anyhow!("unknown pin value (value: {}).", v)),
    };

    let (sender, mut receiver) = unbounded_channel::<Task>();

    let create_opts = mqtt::CreateOptionsBuilder::new()
        .server_uri(format!("tcp://{}", &config.mqtt_host))
        .client_id(&config.mqtt_name_client)
        .finalize();

    let cli = mqtt::AsyncClient::new(create_opts)?;

    let conn_opts = mqtt::ConnectOptionsBuilder::new()
        .keep_alive_interval(Duration::from_secs(30))
        .will_message(mqtt::Message::new_retained(&version_topic, "none", QOS_1))
        .finalize();

    let mqtt_connected_flag = Arc::new(AtomicBool::new(false));

    let mqtt_connect_watcher = cli.clone();

    tokio::spawn(tcp_cub_ip_client(
        config.clone(),
        sender.clone(),
        mqtt_connected_flag.clone(),
    ));

    tokio::spawn(async move {
        while let Err(e) = mqtt_connect_watcher.connect(conn_opts.clone()).await {
            warn!("connect to mqtt failed ({:?})", e);
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        publish_version(version_topic.as_str(), &mqtt_connect_watcher).await;

        mqtt_connected_flag.store(true, Ordering::SeqCst);

        loop {
            if !mqtt_connect_watcher.is_connected() {
                mqtt_connected_flag.store(false, Ordering::SeqCst);
                while let Err(e) = mqtt_connect_watcher.reconnect().await {
                    warn!("mqtt reconnect failed ({:?}).", e);
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                publish_version(version_topic.as_str(), &mqtt_connect_watcher).await;
                mqtt_connected_flag.store(true, Ordering::SeqCst);
                if let Err(e) = sender.send(Task::ResetPrevValue {}) {
                    warn!("ResetPrevValue send to worker mqtt failed ({:?}).", e);
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    });

    let mut prev_temperature_value = None;
    let mut prev_relay_value = None;
    let mut prev_pin_1_value = None;
    let mut prev_pin_2_value = None;

    let response_process = |response: anyhow::Result<CubIpResponse>,
                            prev_temperature_value: &mut Option<String>,
                            prev_relay_value: &mut Option<String>,
                            prev_pin_1_value: &mut Option<String>,
                            prev_pin_2_value: &mut Option<String>| {
        let mut messages = vec![];
        match response {
            Ok(data) => {
                let mut add_message = |topic: &str,
                                       value: &anyhow::Result<String>,
                                       prev_value: &mut Option<String>,
                                       qos: i32| {
                    match value {
                        Ok(v) => {
                            if let Some(prev) = prev_value {
                                if prev == v {
                                    return;
                                }
                            }
                            messages.push(mqtt::Message::new_retained(topic, v.as_bytes(), qos));
                            *prev_value = Some(v.clone());
                        }
                        Err(e) => {
                            warn!("add message: {}", e);
                        }
                    };
                };

                add_message(
                    &temperature_topic,
                    &decode_value_temperature_sensor(data.trm.as_str()),
                    prev_temperature_value,
                    mqtt::QOS_0,
                );

                add_message(
                    &relay_topic,
                    &decode_value_relay_state(data.strel.as_str()),
                    prev_relay_value,
                    mqtt::QOS_1,
                );

                add_message(
                    &pin_1_topic,
                    &decode_value_pin_state(data.vle0.as_str()),
                    prev_pin_1_value,
                    mqtt::QOS_1,
                );

                add_message(
                    &pin_2_topic,
                    &decode_value_pin_state(data.vle1.as_str()),
                    prev_pin_2_value,
                    mqtt::QOS_1,
                );
            }
            Err(e) => {
                warn!("Parsed response failed ({:?}).", e);
            }
        }

        async {
            for msg in messages {
                if let Err(e) = cli.publish(msg).await {
                    warn!("mqtt publish failed ({:?})", e);
                    break;
                }
            }
        }
    };

    loop {
        if let Some(task) = receiver.recv().await {
            match task {
                Task::Response(response) => {
                    response_process(
                        response,
                        &mut prev_temperature_value,
                        &mut prev_relay_value,
                        &mut prev_pin_1_value,
                        &mut prev_pin_2_value,
                    )
                    .await
                }
                Task::ResetPrevValue => {
                    prev_temperature_value = None;
                    prev_relay_value = None;
                    prev_pin_1_value = None;
                    prev_pin_2_value = None;
                }
            }
        }
    }

    Ok(())
}
