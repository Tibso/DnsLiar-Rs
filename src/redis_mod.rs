use crate::{
    enums_structs::{Config, DnsLrResult, WrappedErrors, ErrorKind},
    CONFILE
};

use redis::{
    aio::{ConnectionManager, ConnectionLike},
    Client
};

use tracing::{info, error, warn};
use std::{
    net::{SocketAddr, Ipv4Addr, Ipv6Addr}
};

use trust_dns_client::rr::RecordType;

pub async fn build_manager ()
-> DnsLrResult<ConnectionManager> {
    let client = Client::open(format!("redis://{}/", &CONFILE.redis_address)).expect("Error probing the Redis server");
    info!("{}: Redis server probe successful", &CONFILE.daemon_id);

    let manager = client.get_tokio_connection_manager().await.expect("Error creating the connection manager");
    info!("{}: Connection to Redis successful", &CONFILE.daemon_id);

    return Ok(manager)
}

pub async fn build_config (
    manager: &mut ConnectionManager
)
-> DnsLrResult<Config> {
    let mut config = Config {
        daemon_id: CONFILE.daemon_id.clone(),
        forwarders: vec![],
        binds : vec![],
        is_filtering: false,
        matchclasses: None,
        blackhole_ips: None
    };

    let tmp_blackhole_ips = get(manager, "blackhole_ips", &config.daemon_id).await.expect("Error fetching blackhole_ips");
    let blackhole_ips_count = tmp_blackhole_ips.clone().iter().count();
    if blackhole_ips_count != 2 {
        warn!("{}: Amount of blackhole_ips received were not 2 (must have a v4 and v6)", config.daemon_id);
        warn!("{}: The server will not filter any request and so will not lie", config.daemon_id)
    } else {
        config.blackhole_ips = Some((
            tmp_blackhole_ips[0].parse::<Ipv4Addr>().expect("Error parsing blackhole_ipv4"),
            tmp_blackhole_ips[1].parse::<Ipv6Addr>().expect("Error parsing blackhole_ipv6")
        ));
        info!("{}: Blackhole_ips received are valid", config.daemon_id);

        let tmp_matchclasses = get(manager, "matchclasses", &config.daemon_id).await.expect("Error fetching matchclasses");
        let matchclasses_count = tmp_matchclasses.clone().iter().count();
        if matchclasses_count == 0 {
            warn!("{}: No matchclass received", config.daemon_id);
            warn!("{}: The server will not filter any request and so will not lie", config.daemon_id)
        } else {
            config.is_filtering = true;
            config.matchclasses = Some(tmp_matchclasses);

            info!("{}: Received {} matchclasses", config.daemon_id, matchclasses_count)
        }
    }

    let ser_forwarders = get(manager, "forwarders", &config.daemon_id).await.expect("Error fetching forwarders");
    let forwarders_count = ser_forwarders.clone().iter().count() as u8;
    if forwarders_count == 0 {
        error!("{}: No forwarder was received", config.daemon_id);
        return Err(WrappedErrors::DNSlrError(ErrorKind::SetupForwardersError))
    }
    info!("{}: Received {} forwarders", config.daemon_id, forwarders_count);

    let mut valid_forwarder_count: u8 = 0;
    for forwarder in ser_forwarders {
        config.forwarders.push(
            match forwarder.parse::<SocketAddr>() {
                Ok(ok) => ok,
                Err(_) => {
                    warn!("{}: forwarder: {} is not valid", config.daemon_id, forwarder);
                    continue
                }
            }
        );
        valid_forwarder_count += 1
    }
    if valid_forwarder_count == forwarders_count {
        info!("{}: all {} forwarders are valid", config.daemon_id, valid_forwarder_count)
    } else if valid_forwarder_count < forwarders_count {
        warn!("{}: {} out of {} forwarders are valid", config.daemon_id, valid_forwarder_count, forwarders_count)
    } else if valid_forwarder_count == 0 {
        error!("{}: No forwarder is valid", config.daemon_id);
        return Err(WrappedErrors::DNSlrError(ErrorKind::SetupForwardersError))
    }

    config.binds = get(manager, "binds", &config.daemon_id).await.expect("Error fetching binds");
    let bind_count = config.binds.clone().iter().count() as u32;
    if bind_count == 0 {
        error!("{}: No bind received", config.daemon_id);
        return Err(WrappedErrors::DNSlrError(ErrorKind::SetupBindingError))
    }
    info!("{}: Received {} binds", config.daemon_id, bind_count);

    return Ok(config)
}

pub async fn exists (
    manager: &mut ConnectionManager,
    fullmatch: String,
    qtype: RecordType
)
-> DnsLrResult<bool> {
    let qtype: &str = match qtype {
        RecordType::A => "A", 
        RecordType::AAAA => "AAAA",
        _ => unreachable!()
    };

    let ser_answer = manager.req_packed_command(
        redis::Cmd::new()
            .arg("EXISTS")
            .arg(fullmatch)
            .arg(qtype))
            .await?;
    
    let deser_answer = redis::FromRedisValue::from_redis_value(&ser_answer)?;
    return Ok(deser_answer)
}

pub async fn get (
    manager: &mut ConnectionManager,
    kind: &str,
    daemon_id: &String
)
-> DnsLrResult<Vec<String>> {
    let ser_answer = manager.req_packed_command(
        redis::Cmd::new()
            .arg("HKEYS")
            .arg(format!("{}_{}", kind, daemon_id)))
            .await?;

    let deser_answer = redis::FromRedisValue::from_redis_value(&ser_answer)?;
    return Ok(deser_answer)
}
