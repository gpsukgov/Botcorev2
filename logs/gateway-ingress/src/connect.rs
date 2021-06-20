//! Contains utility functions that connect to external services,
//! used during service initialization and during potential reconnection

use crate::config::Configuration;
use crate::rpc::feature_gate::Client as FeatureGateClient;
use crate::rpc::logs::uptime::Client as LogsUptimeClient;
use crate::INTENTS;
use anyhow::{Context, Result};
use lapin::{Connection, ConnectionProperties};
use slog::Logger;
use std::sync::Arc;
use twilight_gateway::shard::Events;
use twilight_gateway::{EventTypeFlags, Shard};

/// Attempts to initialize a gateway connection
pub async fn to_shard(
    config: Arc<Configuration>,
    logger: Logger,
    events: EventTypeFlags,
) -> Result<(Shard, Events)> {
    let initialization_backoff = config.initialization_backoff.build();
    let shard_connect = || async {
        let (shard, events) = Shard::builder(config.secrets.discord_token.clone(), *INTENTS)
            .event_types(events)
            .build();
        shard.start().await.map_err(|err| {
            slog::warn!(
                logger,
                "couldn't start bot shard, retrying after backoff";
                "error" => ?err,
            );
            err
        })?;
        Ok((shard, events))
    };
    let (shard, events) = backoff::future::retry(initialization_backoff, shard_connect)
        .await
        .context("could not start shard")?;
    slog::info!(
        logger,
        "created shard and preparing to listen for gateway events"
    );
    Ok((shard, events))
}

/// Creates a new connection to Rabbit MQ
pub async fn to_queue(config: Arc<Configuration>, logger: Logger) -> Result<Connection> {
    let initialization_backoff = config.initialization_backoff.build();
    let rmq_url = config.services.gateway_queue.clone();
    let rmq_connect = || async {
        let conn = Connection::connect(&rmq_url, ConnectionProperties::default())
            .await
            .map_err(|err| {
                slog::warn!(
                    logger,
                    "couldn't connect to RabbitMQ, retrying after backoff";
                    "rabbit_url" => rmq_url,
                    "error" => ?err,
                );
                err
            })?;
        Ok(conn)
    };
    let rmq_connection = backoff::future::retry(initialization_backoff, rmq_connect)
        .await
        .context("could not connect to the RabbitMQ gateway queue")?;
    slog::info!(logger, "connected to RabbitMQ"; "rabbit_url" => rmq_url);
    Ok(rmq_connection)
}

/// Creates a new connection to the feature gate service
pub async fn to_feature_gate(
    config: Arc<Configuration>,
    logger: Logger,
) -> Result<FeatureGateClient> {
    let initialization_backoff = config.initialization_backoff.build();
    let feature_gate_url = config.services.feature_gate.clone();
    let connect = || async {
        let conn = FeatureGateClient::connect(feature_gate_url.clone())
            .await
            .map_err(|err| {
                slog::warn!(
                    logger,
                    "couldn't connect to feature-gate, retrying after backoff";
                    "feature_gate_url" => feature_gate_url,
                    "error" => ?err,
                );
                err
            })?;
        Ok(conn)
    };
    let connection = backoff::future::retry(initialization_backoff, connect)
        .await
        .context("could not connect to feature-gate")?;
    slog::info!(logger, "connected to feature-gate"; "feature_gate_url" => feature_gate_url);
    Ok(connection)
}

/// Creates a new connection to the logs/uptime service
pub async fn to_uptime_service(
    config: Arc<Configuration>,
    logger: Logger,
) -> Result<LogsUptimeClient> {
    let initialization_backoff = config.initialization_backoff.build();
    let uptime_url = config.services.logs_uptime.clone();
    let connect = || async {
        let conn = LogsUptimeClient::connect(uptime_url.clone())
            .await
            .map_err(|err| {
                slog::warn!(
                    logger,
                    "couldn't connect to logs/uptime, retrying after backoff";
                    "logs_uptime_url" => uptime_url,
                    "error" => ?err,
                );
                err
            })?;
        Ok(conn)
    };
    let connection = backoff::future::retry(initialization_backoff, connect)
        .await
        .context("could not connect to logs/uptime")?;
    slog::info!(logger, "connected to logs/uptime"; "logs_uptime_url" => uptime_url);
    Ok(connection)
}