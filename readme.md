# 🔮 CubIpApp

CubIpApp is a bridge between
the [KubIp device](https://ttronics.ru/products/kontrollery/umnii-biznes/Kontroller-KUB-IP/) and the mqtt broker.

## ⚙ Feature

- 🌡 Temperature from external sensor
- ⚡ Relay control
- ⏲ Time-relay control
- ⬇ Status of input pins

## 🦟 MQTT

CubIpApp will publish status changes in the following topics for MQTT clients:

| Topic                               | Description                                                                                                            |
|-------------------------------------|------------------------------------------------------------------------------------------------------------------------|
| /{mqtt_main_topic}/temperature      | Temperature value from an external sensor                                                                              |
| /{mqtt_main_topic}/relay/state      | Relay state (0 - off, 1 - on)                                                                                          |
| /{mqtt_main_topic}/time_relay/state | Timer value (If the relay is on, it will turn off when the timer expires and vice versa)                               |
| /{mqtt_main_topic}/pin_1/state      | Input pin 1 status (0 - open, 1 - closed)                                                                              |
| /{mqtt_main_topic}/pin_2/state      | Input pin 2 status (0 - open, 1 - closed)                                                                              |
| /{mqtt_main_topic}/version          | version application ( none - no connection to the service, symbol "-" - no connection to the device, "x.x.x" - version |

CubIpApp will subscribe to the following topics to change relay status:

| Topic                                      | Description                                     |
|--------------------------------------------|-------------------------------------------------|
| /{mqtt_main_topic}/relay/cmd               | Send 0 or 1 to turn the relay on or off         |
| /{mqtt_main_topic}/time_relay/cmd/turn_on  | Send the delay in seconds to turn on the relay  |
| /{mqtt_main_topic}/time_relay/cmd/turn_off | Send the delay in seconds to turn off the relay |

{mqtt_main_topic} - mqtt_main_topic value from configuration file

## 🔩 Configuration

CubIpApp is configured through the configuration file (/etc/cub_ip_app.toml)

| Field                     | Description                                       |
|---------------------------|---------------------------------------------------|
| cub_ip_address            | KubIp device ip                                   |
| cub_ip_login              | login for the web interface                       |
| cub_ip_password           | password for the web interface                    |
| cub_ip_connection_timeout | connection timeout to KubIp                       |
| cub_ip_read_timeout       | socket read operation timeout (KubIp connection)  |
| cub_ip_write_timeout      | socket write operation timeout (KubIp connection) |
| cub_ip_request_timeout    | timeout between status requests to KubIp          |
| mqtt_main_topic           | top mqtt topic                                    |
| mqtt_host                 | mqtt broker ip                                    |
| mqtt_name_client          | mqtt client name                                  |
| log_level                 | log level                                         |

## 🏗️ Build

[RaspberryPi build](raspberry_pi_build/readme.md)
