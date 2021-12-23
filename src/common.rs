use serde::Deserialize;

#[derive(Debug, Deserialize, PartialEq)]
pub struct CubIpResponse {
    pub tm: String,
    pub stv: String,
    pub trm: String, // temperature
    pub hih: String,
    pub vle0: String, // pin 1
    pub vle1: String, // pin 2
    pub vle2: String,
    pub vle3: String,
    pub vle4: String,
    pub vle5: String,
    pub strel: String, // relay 0: turn off; 1: turn on;  2: turn off (time relay) 3: turn on (time relay)
    pub tmrel: String, // time relay
}

#[derive(Debug)]
pub enum CubIpRelayAction {
    RelayTurnOn,
    RelayTurnOff,
    TimeRelayTurnOn(usize),
    TimeRelayTurnOff(usize),
}

#[derive(Debug)]
pub enum Task {
    Response(Box<anyhow::Result<CubIpResponse>>),
    ResetPrevValue,
}
