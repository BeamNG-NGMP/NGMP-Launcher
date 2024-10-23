#[macro_use] extern crate log;

use std::net::SocketAddr;

use ngmp_protocol_impl::{
    connection::*,
    launcher_client,
};

use ngmp_protocol_impl::launcher_client::Packet as ClientPacket;

mod logger;
mod config;
mod signal;
mod http;
mod server_conn;

use signal::{Signal, SignalReceiver};

use server_conn::launcher_on_server_main;

#[tokio::main]
async fn main() {
    logger::init(log::LevelFilter::max(), true).expect("Failed to initialize logger!");
    info!("Logger initialized!");

    let config = config::load_config();

    // When the main server thread stops running, this signal is automatically dropped.
    let (kill_signal, kill_receiver) = Signal::new();
    {
        let signal_rx = kill_receiver.clone();
        std::thread::spawn(move || http::http_main(signal_rx, config.networking.http_port));
    }

    // Start the UDP listener for the client - launcher connection
    let udp_bind_addr = format!("127.0.0.1:{}", config.networking.launcher_port);
    let mut launcher_listener = UdpListener::<ClientPacket>::bind(&udp_bind_addr).await.map_err(|e| error!("{}", e)).expect("Failed to bind to UDP address!");
    loop {
        info!("Waiting for game to connect on port {}!", config.networking.launcher_port);
        match launcher_listener.wait_for_packet().await {
            Ok((packet, local_addr)) => match packet {
                ClientPacket::ClientInfo(client_info) => launcher_main(&mut launcher_listener, local_addr, client_info, kill_receiver.clone()).await,
                _ => error!("Unknown packet sent! Packet: {:?}", packet),
            },
            Err(e) => error!("Failed to receive packet from client: {}", e),
        }
    }
}

async fn launcher_main(
    launcher_socket: &mut UdpListener<ClientPacket>,
    local_addr: SocketAddr,
    client_info: launcher_client::handshake::ClientInfoPacket,
    signal_rx: SignalReceiver
) {
    info!("Game connected!");

    launcher_socket.write_packet(local_addr, ClientPacket::Version(launcher_client::handshake::VersionPacket {
        confirm_id: 1,
        protocol_version: 1,
    })).await;

    launcher_socket.write_packet(local_addr, ClientPacket::AuthenticationInfo(launcher_client::handshake::AuthenticationInfoPacket {
        confirm_id: 2,
        success: true,
        player_name: "mista gaming".to_string(),
    })).await;

    loop {
        match launcher_socket.wait_for_packet().await {
            Ok((packet, _addr)) => match packet {
                ClientPacket::ReloadLauncherConnection => {
                    info!("Client mod disconnected! Waiting for it to reconnect...");
                    break;
                },
                ClientPacket::JoinServer(p) => {
                    match p.ip_addr.parse::<SocketAddr>() {
                        Ok(server_addr) => {
                            if let Err(e) = launcher_on_server_main(
                                launcher_socket,
                                local_addr,
                                &client_info,
                                signal_rx.clone(),
                                server_addr,
                            ).await {
                                error!("Error in server conn: {}", e);
                                // TODO: Report back to client.
                            }
                            // When this returns, we are done playing on the server, so we can
                            // just break and reconnect to the game.
                            break;
                        },
                        Err(e) => {
                            error!("{}", e);
                            todo!(); // Respond to client with error packet
                        }
                    }
                },
                _ => error!("Unknown packet! {:#?}", packet),
            },
            Err(e) => {
                error!("Failed to receive packet from client: {}", e);
                break;
            },
        }
    }
}
