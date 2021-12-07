use anyhow::{anyhow, Context};
use futures::stream::StreamExt;
use log::{info, warn};
use std::sync::Arc;
use std::time::Duration;
use tokio::io;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

extern crate quick_xml;
extern crate serde;
#[macro_use]
extern crate clap;

use paho_mqtt as mqtt;
use paho_mqtt::QOS_1;
use quick_xml::de::from_str;
use serde::{de, Deserialize, Deserializer};

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

#[derive(Debug)]
enum CubIpRelayAction {
    RelayTurnOn,
    RelayTurnOff,
    TimeRelayTurnOn(usize),
    TimeRelayTurnOff(usize),
}

#[derive(Deserialize)]
struct Config {
    pub cub_ip_address: String,
    pub cub_ip_login: String,
    pub cub_ip_password: String,
    #[serde(deserialize_with = "deserialize_duration_from_str")]
    pub cub_ip_connection_timeout: Duration,
    #[serde(deserialize_with = "deserialize_duration_from_str")]
    pub cub_ip_read_timeout: Duration,
    #[serde(deserialize_with = "deserialize_duration_from_str")]
    pub cub_ip_write_timeout: Duration,
    #[serde(deserialize_with = "deserialize_duration_from_str")]
    pub cub_ip_request_timeout: Duration,
    pub mqtt_main_topic: String,
    pub mqtt_host: String,
    pub mqtt_name_client: String,
}

fn deserialize_duration_from_str<'de, D>(deserializer: D) -> Result<Duration, D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    parse_duration::parse(s.as_str()).map_err(de::Error::custom)
}

impl Config {
    pub fn read_from_file(path: &str) -> anyhow::Result<Config> {
        let mut settings = config::Config::default();
        settings
            .merge(config::File::with_name(path))
            .with_context(|| format!("Config::read_from_file(): open file {}", path))?;

        settings
            .get("main")
            .with_context(|| "Config::read_from_file(): deserialize")
    }
}

async fn request_to_cub_ip(config: &Arc<Config>, path: &str) -> anyhow::Result<Vec<u8>> {
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

async fn request_relay_action(
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
        &config,
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

async fn request_status(
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

async fn subscribe_cmd(
    config: Arc<Config>,
    relay_cmd_topic: String,
    time_relay_turn_on_cmd_topic: String,
    time_relay_turn_off_cmd_topic: String,
    mqtt_client: mqtt::AsyncClient,
    mut strm: futures::channel::mpsc::Receiver<Option<mqtt::Message>>,
) {
    while !mqtt_client.is_connected() {
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    while let Err(e) = mqtt_client
        .subscribe_many(
            &[
                &relay_cmd_topic,
                &time_relay_turn_on_cmd_topic,
                &time_relay_turn_off_cmd_topic,
            ],
            &[mqtt::QOS_1, mqtt::QOS_1, mqtt::QOS_1],
        )
        .await
    {
        warn!("subscribe on mqtt topic failed ({:?}).", e);
    }

    while let Some(msg_opt) = strm.next().await {
        if let Some(msg) = msg_opt {
            // let payload = std::str::from_utf8(msg.payload().);
            let to_usize = |time_as_str: &[u8]| {
                std::str::from_utf8(time_as_str)
                    .map(|v| v.parse::<usize>().ok())
                    .ok()
                    .flatten()
            };

            let action = if msg.topic() == relay_cmd_topic {
                match msg.payload() {
                    b"0" => Some(CubIpRelayAction::RelayTurnOff),
                    b"1" => Some(CubIpRelayAction::RelayTurnOn),
                    _ => None,
                }
            } else if msg.topic() == time_relay_turn_on_cmd_topic {
                to_usize(msg.payload()).map(|v| CubIpRelayAction::TimeRelayTurnOn(v))
            } else if msg.topic() == time_relay_turn_off_cmd_topic {
                to_usize(msg.payload()).map(|v| CubIpRelayAction::TimeRelayTurnOff(v))
            } else {
                None
            };

            if let Some(action) = action {
                if let Err(e) = request_relay_action(&config, action).await {
                    warn!("request_relay_action failed ({:?})", e);
                }
                continue;
            }
            warn!("unknown cmd {}.", msg);
        } else {
            // A "None" means we were disconnected. Wait for reconnect...
            while !mqtt_client.is_connected() {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            info!("mqtt cmd reconnect recovery");
        }
    }
    warn!("subscribe loop exit.");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let matches = clap_app!(CubIpApp =>
        (@arg CONFIG: -c --config +takes_value "Sets a custom config file"))
    .get_matches();

    let path_to_config = matches
        .value_of("CONFIG")
        .unwrap_or_else(|| "/usr/local/etc/cub_ip_app/config.toml");

    let config = Arc::new(Config::read_from_file(path_to_config)?);

    let temperature_topic = format!("/{}/temperature", config.mqtt_main_topic);
    let relay_topic = format!("/{}/relay/state", config.mqtt_main_topic);
    let relay_cmd_topic = format!("/{}/relay/cmd", config.mqtt_main_topic);
    let relay_time_topic = format!("/{}/time_relay/state", config.mqtt_main_topic);
    let time_relay_turn_on_cmd_topic =
        format!("/{}/time_relay/cmd/turn_on", config.mqtt_main_topic);
    let time_relay_turn_off_cmd_topic =
        format!("/{}/time_relay/cmd/turn_off", config.mqtt_main_topic);
    let pin_1_topic = format!("/{}/pin_1/state", config.mqtt_main_topic);
    let pin_2_topic = format!("/{}/pin_2/state", config.mqtt_main_topic);
    let version_topic = format!("/{}/version", config.mqtt_main_topic);

    let decode_value_temperature_sensor = |v: &str| {
        if v == "Обрыв датчика" || v == "-" {
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
        "-" => Ok(v.to_string()),
        _ => Err(anyhow!("unknown relay value (value: {}).", v)),
    };

    let decode_value_pin_state = |v: &str| match v {
        "Сработка" => Ok("0".to_string()),
        "Норма" => Ok("1".to_string()),
        "-" => Ok(v.to_string()),
        _ => Err(anyhow!("unknown pin value (value: {}).", v)),
    };

    let (sender, mut receiver) = unbounded_channel::<Task>();

    let create_opts = mqtt::CreateOptionsBuilder::new()
        .server_uri(format!("tcp://{}", &config.mqtt_host))
        .client_id(&config.mqtt_name_client)
        .finalize();

    let mut cli = mqtt::AsyncClient::new(create_opts)?;

    let conn_opts = mqtt::ConnectOptionsBuilder::new()
        .keep_alive_interval(Duration::from_secs(20))
        .clean_session(false)
        .will_message(mqtt::Message::new_retained(&version_topic, "none", QOS_1))
        .finalize();

    let mqtt_connect_watcher = cli.clone();

    tokio::spawn(request_status(config.clone(), sender.clone(), cli.clone()));

    tokio::spawn(subscribe_cmd(
        config,
        relay_cmd_topic,
        time_relay_turn_on_cmd_topic,
        time_relay_turn_off_cmd_topic,
        cli.clone(),
        cli.get_stream(25),
    ));

    tokio::spawn(async move {
        while let Err(e) = mqtt_connect_watcher.connect(conn_opts.clone()).await {
            warn!("connect to mqtt failed ({:?})", e);
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        publish_version(version_topic.as_str(), &mqtt_connect_watcher).await;

        loop {
            if !mqtt_connect_watcher.is_connected() {
                while let Err(e) = mqtt_connect_watcher.reconnect().await {
                    warn!("mqtt reconnect failed ({:?}).", e);
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                publish_version(version_topic.as_str(), &mqtt_connect_watcher).await;
                info!("mqtt reconnect recovery");
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
    let mut prev_time_relay_value = None;

    let response_process = |response: anyhow::Result<CubIpResponse>,
                            prev_temperature_value: &mut Option<String>,
                            prev_relay_value: &mut Option<String>,
                            prev_pin_1_value: &mut Option<String>,
                            prev_pin_2_value: &mut Option<String>,
                            prev_time_relay_value: &mut Option<String>| {
        let mut messages = vec![];

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

        let data = response.unwrap_or_else(|e| {
            warn!("Parsed response failed ({:?}).", e);
            CubIpResponse {
                tm: "-".to_string(),
                stv: "-".to_string(),
                trm: "-".to_string(),
                hih: "-".to_string(),
                vle0: "-".to_string(),
                vle1: "-".to_string(),
                vle2: "-".to_string(),
                vle3: "-".to_string(),
                vle4: "-".to_string(),
                vle5: "-".to_string(),
                strel: "-".to_string(),
                tmrel: "-".to_string(),
            }
        });

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
        add_message(
            &relay_time_topic,
            &Ok::<String, anyhow::Error>(data.tmrel),
            prev_time_relay_value,
            mqtt::QOS_1,
        );

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
                        &mut prev_time_relay_value,
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

#[cfg(test)]
mod test {
    use crate::{request_relay_action, Config, CubIpRelayAction};
    use std::sync::Arc;

    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn action_request() {
        env_logger::init();

        let config = Arc::new(Config::read_from_file(""));

        request_relay_action(&config, CubIpRelayAction::TimeRelayTurnOn(15))
            .await
            .unwrap();
    }
}
