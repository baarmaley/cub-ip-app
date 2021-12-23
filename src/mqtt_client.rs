use crate::common::CubIpRelayAction;
use crate::config::Config;
use crate::cub_ip_client::request_relay_action;
use crate::mqtt;
use futures::StreamExt;
use log::{info, warn};
use paho_mqtt::QOS_1;
use std::sync::Arc;
use std::time::Duration;

pub async fn subscribe_cmd(
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
                to_usize(msg.payload()).map(CubIpRelayAction::TimeRelayTurnOn)
            } else if msg.topic() == time_relay_turn_off_cmd_topic {
                to_usize(msg.payload()).map(CubIpRelayAction::TimeRelayTurnOff)
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

pub async fn publish_version(version_topic: &str, mqtt_client: &mqtt::AsyncClient) {
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
