extern crate websocket;

use websocket::client::ClientBuilder;
use websocket::Message;
use websocket::OwnedMessage;
use serde::{Deserialize, Serialize};
use serde::de::{self, Deserializer, Unexpected, Visitor};
// use serde_json::{Result};
use chrono::{DateTime, Utc, Timelike};
use uuid::Uuid;
use std::fmt;

const CONNECTION: &'static str = "wss://ws-feed.exchange.coinbase.com";

fn main() {
	println!("Connecting to {}", CONNECTION);

	let mut client = ClientBuilder::new(CONNECTION)
		.unwrap()
		.connect(None)
		.unwrap();

	println!("Successfully connected");

	let start_message = Message::text(r#"{
		"type": "subscribe",
		"product_ids": [
			"ETH-USD"
		],
		"channels": [
			"full"
		]
	}"#);
	
	client.send_message(&start_message)
		.unwrap();

	loop {
		let incoming_message = client.recv_message()
			.unwrap();

		process_owned_message(incoming_message);
	}
}

fn process_owned_message(message: OwnedMessage) {
	match message {
		OwnedMessage::Text(x) => process_coinbase_message(x),
		OwnedMessage::Binary(_) => println!("binary"),
		OwnedMessage::Close(_) => println!("close"),
		OwnedMessage::Ping(_) => println!("ping"),
		OwnedMessage::Pong(_) => println!("pong"),
	}
}

#[derive(Serialize, Deserialize)]
struct CoinbaseReceived {
	order_id: Uuid,
	#[serde(alias = "type")]
	message_type: String,
    time: DateTime<Utc>,
	#[serde(deserialize_with = "string_as_f64")]
	price: f64,
	side: String,
}

fn process_coinbase_message(message: String) {
	//println!("{}", message);
	let parsed_message: serde_json::Result<CoinbaseReceived> = serde_json::from_str(&message);

	match parsed_message {
		Result::Ok(v) => println!("order_id {} type {} side {} price {}", v.order_id, v.message_type, v.side, v.price),
		Result::Err(e) => println!("{}", e)
	}
}

fn string_as_f64<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_str(F64Visitor)
}

struct F64Visitor;
impl<'de> Visitor<'de> for F64Visitor {
    type Value = f64;
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a string representation of a f64")
    }
    fn visit_str<E>(self, value: &str) -> Result<f64, E>
    where
        E: de::Error,
    {
        value.parse::<f64>().map_err(|_err| {
            E::invalid_value(Unexpected::Str(value), &"a string representation of a f64")
        })
    }
}