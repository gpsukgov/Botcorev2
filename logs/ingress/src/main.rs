mod config;
mod event;
mod gateway;

use crate::event::NormalizedEvent;
use crate::gateway::OriginalEvent;
use anyhow::{Context, Result};
use clap::{App, Arg};
use config::Configuration;
use futures::{Stream, StreamExt};
use lazy_static::lazy_static;
use log::{debug, error, info};
use logs_lib::time;
use std::convert::TryFrom;
use std::future::Future;
use std::sync::Arc;
use twilight_gateway::{Event, EventTypeFlags, Intents, Shard};
use twilight_model::gateway::event::gateway::GatewayEventDeserializer;
use twilight_model::gateway::event::shard::Payload;
use twilight_model::gateway::OpCode;

/// Bootstraps the bot and begins listening for gateway events
#[tokio::main]
async fn main() {
    env_logger::init();
    match run().await {
        Ok(_) => info!("Exiting"),
        Err(err) => error!("{:?}", err),
    }
}

lazy_static! {
    /// Includes all guild-related events to signal to Discord that we intend to
    /// receive and process them
    pub static ref INTENTS: Intents = Intents::GUILDS
        | Intents::GUILD_MEMBERS
        | Intents::GUILD_BANS
        | Intents::GUILD_EMOJIS
        | Intents::GUILD_INTEGRATIONS
        | Intents::GUILD_WEBHOOKS
        | Intents::GUILD_INVITES
        | Intents::GUILD_VOICE_STATES
        | Intents::GUILD_MEMBERS
        | Intents::GUILD_MESSAGES
        | Intents::GUILD_MESSAGE_REACTIONS;
}

/// Attempts to initialize the bot and listen for gateway events
async fn run() -> Result<()> {
    // Use clap to pass in a config path
    let app = App::new("logs-ingress").arg(
        Arg::with_name("config")
            .short("c")
            .long("config")
            .value_name("FILE")
            .help("TOML config file path")
            .takes_value(true)
            .required(true),
    );
    let matches = app.get_matches();
    let config_path = matches.value_of("config").unwrap();

    // Parse the config from the path and use it to initialize the event stream
    let config = Configuration::try_load(config_path)?;
    let event_types = EventTypeFlags::SHARD_PAYLOAD;
    let mut shard = Shard::new(config.secrets.discord_token, *INTENTS);
    let events = shard.some_events(event_types);

    // Initialize the gateway event processor
    // and register all known gateway event handlers
    // (see gateway/processors.rs)
    let processor = Arc::new(gateway::sub_processors::register_all(
        gateway::Processor::new(),
    ));

    shard.start().await.context("Could not start shard")?;
    info!("Created shard and preparing to listen for gateway events");

    // Listen for all raw gateway events and process them,
    // re-emitting half-processed gateway events to be consumed by the processor
    let gateway_event_stream = pipe_gateway_events(events, Arc::clone(&processor));

    // Normalize each event coming from the gateway,
    // and process them in parallel where possible via buffer_unordered
    let normalized_event_stream =
        pipe_normalized_events(gateway_event_stream, Arc::clone(&processor))
            .buffer_unordered(config.normalization_stream_concurrency)
            .filter_map(|event_option| async move { event_option });

    // Send each normalized event to the logging import service,
    // acting as a sink for this stream
    normalized_event_stream
        .for_each_concurrent(Some(config.import_stream_concurrency), |event| async move {
            // TODO implement sending via gRPC to import service
            if let Some(s) = serde_json::to_string_pretty(&event).ok() {
                info!("Normalized event received at sink: {}", s);
            }
        })
        .await;

    Ok(())
}

/// Stream processor function that takes in a raw stream of gateway events
/// and uses twilight's fast pre-deserializer to validate that
/// they are valid and usable events before parsing and re-emitting them
fn pipe_gateway_events(
    in_stream: impl Stream<Item = Event>,
    processor: Arc<gateway::Processor>,
) -> impl Stream<Item = OriginalEvent> {
    // Get the opcode byte number for `OpCode::Event` packets
    let event_opcode: u8 =
        match serde_json::to_value(OpCode::Event).expect("Couldn't turn OpCode::Event into json") {
            serde_json::Value::Number(n) => n
                .as_u64()
                .and_then(|i| TryFrom::try_from(i).ok())
                .expect("Couldn't turn OpCode::Event into u8"),
            _ => panic!("serialization from OpCode produced non-u8"),
        };

    in_stream.filter_map(move |event| {
        let processor_copy = Arc::clone(&processor);
        async move {
            if let Event::ShardPayload(Payload { bytes }) = event {
                let json = std::str::from_utf8(&bytes).ok()?;
                // Use twilight's fast pre-deserializer to determine the op type,
                // and only deserialize it if it:
                // - is a proper Gateway dispatch event
                // - has a matching processor
                let deserializer = GatewayEventDeserializer::from_json(json)?;
                let (op, seq, event_type) = deserializer.into_parts();
                if op != event_opcode {
                    return None;
                }

                // Make sure we can process the event
                let event_type = event_type.as_deref()?;
                if !processor_copy.can_process(event_type) {
                    return None;
                }

                let value = serde_json::from_str::<serde_json::Value>(json).ok()?;
                if let serde_json::Value::Object(map) = value {
                    // Attempt to find the ".d" value (contains the Gateway message payload)
                    // https://discord.com/developers/docs/topics/gateway#payloads-gateway-payload-structure
                    let mut map = map;
                    let inner_json = map.remove("d")?;
                    return Some(OriginalEvent {
                        seq,
                        event_type: event_type.to_owned(),
                        json: inner_json,
                        rx_timestamp: time::millisecond_ts(),
                    });
                }
            }

            None
        }
    })
}

/// Stream processor function that invokes the core event processing logic
/// on each incoming original gateway event,
/// attempting to asynchronously convert them into NormalizedEvents
fn pipe_normalized_events(
    in_stream: impl Stream<Item = OriginalEvent>,
    processor: Arc<gateway::Processor>,
) -> impl Stream<Item = impl Future<Output = Option<NormalizedEvent>>> {
    in_stream.map(move |event| {
        let processor = Arc::clone(&processor);
        async move {
            match processor.normalize(event).await {
                Ok(normalized_event) => Some(normalized_event),
                Err(err) => {
                    debug!("Event normalization failed for event: {:?}", err);
                    None
                }
            }
        }
    })
}