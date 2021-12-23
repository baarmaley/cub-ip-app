mod common;
mod config;
mod cub_ip_client;
mod mqtt_client;

use anyhow::anyhow;
use log::{info, warn};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::unbounded_channel;

extern crate quick_xml;
extern crate serde;
#[macro_use]
extern crate clap;

use crate::common::{CubIpResponse, Task};
use crate::config::Config;
use crate::cub_ip_client::request_status;
use crate::mqtt_client::{publish_version, subscribe_cmd};
use paho_mqtt as mqtt;
use paho_mqtt::QOS_1;
use syslog::Facility;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let matches = clap_app!(CubIpApp =>
        (@arg CONFIG: -c --config +takes_value "Sets a custom config file"))
    .get_matches();

    let path_to_config = matches
        .value_of("CONFIG")
        .unwrap_or("/usr/local/etc/cub_ip_app/config.toml");

    let config = Arc::new(Config::read_from_file(path_to_config)?);

    syslog::init(Facility::LOG_USER, config.log_level, None).unwrap();

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
                        *response,
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
}

#[cfg(test)]
mod test {
    use crate::common::CubIpRelayAction;
    use crate::cub_ip_client::request_relay_action;
    use crate::Config;
    use std::sync::Arc;

    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn action_request() {
        env_logger::init();

        let config = Arc::new(Config::read_from_file("").unwrap());

        request_relay_action(&config, CubIpRelayAction::TimeRelayTurnOn(15))
            .await
            .unwrap();
    }
}
