mod handler_mod;
mod redis_mod;
mod resolver_mod;
mod matching;
mod enums_structs;

use crate::handler_mod::Handler;
use crate::enums_structs::{Config, DnsLrResult, WrappedErrors, ErrorKind, Confile};

use arc_swap::ArcSwap;
use trust_dns_server::ServerFuture;

use tokio::{
    net::{TcpListener, UdpSocket}
};
use std::{
    time::Duration,
    fs,
    sync::Arc
};
use tracing::{info, error, warn};
use signal_hook_tokio::Signals;
use signal_hook::consts::signal::{SIGHUP, SIGUSR1, SIGUSR2};
use futures_util::{
    stream::StreamExt
};
use lazy_static::lazy_static;

const TCP_TIMEOUT: Duration = Duration::from_secs(10);

lazy_static! {
    static ref CONFILE: Confile = read_confile("dnslr.conf");
}

fn read_confile (
    file_name: &str
)
-> Confile {
    let confile: Confile = {
        let data = fs::read_to_string(file_name).expect("Error reading config file");
        serde_json::from_str(&data).expect("Error deserializing config file data")
    };

    info!("Daemon_id is {}", confile.daemon_id);
    info!("{}: Redis server: {}", confile.daemon_id, confile.redis_address);
    
    return confile
}

async fn setup_binds (
    server: &mut ServerFuture<Handler>,
    config: &Config
)
-> DnsLrResult<()> {
    let bind_count = config.binds.clone().iter().count() as u32;
    let mut successful_binds_count: u32 = 0;
    for bind in config.binds.clone().into_iter() {
        let splits: Vec<&str> = bind.split("=").collect();

        match splits[0] {
            "UDP" => {
                let Ok(socket) = UdpSocket::bind(splits[1]).await else {
                    warn!("{}: Failed to bind: {}", config.daemon_id, bind);
                    continue
                };
                server.register_socket(socket)
            },
            "TCP" => {
                let Ok(listener) = TcpListener::bind(splits[1]).await else {
                    warn!("{}: Failed to bind: {}", config.daemon_id, bind);
                    continue
                };
                server.register_listener(listener, TCP_TIMEOUT)
            },
            _ => {
                warn!("{}: Failed to bind: {}", config.daemon_id, bind);
                continue
            }
        };
        successful_binds_count += 1
    }
    if successful_binds_count == bind_count {
        info!("{}: all {} binds were set", config.daemon_id, successful_binds_count)
    } else if successful_binds_count < bind_count {
        warn!("{}: {} out of {} total binds were set", config.daemon_id, successful_binds_count, bind_count)
    } else if successful_binds_count == 0 {
        error!("{}: No bind was set", config.daemon_id);
        return Err(WrappedErrors::DNSlrError(ErrorKind::SetupBindingError))
    }

    return Ok(())
}

async fn handle_signals (
    mut signals: Signals,
    arc_config: Arc<ArcSwap<Config>>,
    mut redis_manager: redis::aio::ConnectionManager
) {
    while let Some(signal) = signals.next().await {
        match signal {
            SIGHUP => {
                info!("Captured SIGHUP");

                let Ok(new_config) = redis_mod::build_config(&mut redis_manager).await else {
                    error!("Could not rebuild the config");
                    continue
                };
                let new_config =  Arc::new(new_config);
                arc_config.store(new_config);
                info!("Config was rebuilt")
            },
            SIGUSR1 => {
                info!("Captured SIGUSR1");
            },
            SIGUSR2 => {
                info!("Captured SIGUSR2");

            },
            _ => unreachable!()
        }
    }
}

#[tokio::main]
async fn main()
-> DnsLrResult<()> {
    tracing_subscriber::fmt::init();

    let signals = Signals::new(&[SIGHUP, SIGUSR1, SIGUSR2])?;
    let signals_handler = signals.handle();

    let mut redis_manager = redis_mod::build_manager().await?;
    let config = redis_mod::build_config(&mut redis_manager).await?;
    let resolver = resolver_mod::build_resolver(&config);

    info!("{}: Initializing server...", config.daemon_id);
    let arc_config = Arc::new(ArcSwap::from_pointee(config.clone()));

    let handler = Handler {
        redis_manager: redis_manager.clone(), resolver, config: Arc::clone(&arc_config)
    };
    
    let signals_task = tokio::task::spawn(handle_signals(signals, Arc::clone(&arc_config), redis_manager));

    let mut server = ServerFuture::new(handler);

    setup_binds(&mut server, &config).await?;

    info!("{}: Server started", config.daemon_id);
    server.block_until_done().await?;

    signals_handler.close();
    signals_task.await?;

    return Ok(())
}
