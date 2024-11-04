#[macro_use]
extern crate log;

use std::net::SocketAddr;
use tokio::sync::mpsc;

use ngmp_protocol_impl::{connection::*, launcher_client};

use ngmp_protocol_impl::launcher_client::Packet as ClientPacket;

mod config;
mod http;
mod logger;
mod server_conn;
mod signal;

use signal::{Signal, SignalReceiver};

use server_conn::launcher_on_server_main;

#[tokio::main]
async fn main() {
    logger::init(log::LevelFilter::max(), true).expect("Failed to initialize logger!");
    info!("Logger initialized!");

    let config = config::load_config();

    // Set up a messaging channel for HTTP->Launcher messages
    let (http_msg_tx, mut http_msg_rx) = mpsc::channel(25);

    // When the main server thread stops running, this signal is automatically dropped.
    let (kill_signal, kill_receiver) = Signal::new();
    {
        let signal_rx = kill_receiver.clone();
        std::thread::spawn(move || {
            http::http_main(signal_rx, config.networking.http_port, http_msg_tx)
        });
    }

    let bind_addr = format!("127.0.0.1:{}", config.networking.launcher_port);
    let tcp_listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .expect("Failed to bind TCP socket!");

    loop {
        match tcp_listener.accept().await {
            Ok((socket, _addr)) => {
                let mut tcp_conn = TcpConnection::<ClientPacket>::from_stream(socket);
                match tcp_conn.wait_for_packet().await {
                    Ok(ClientPacket::ClientInfo(client_info)) => {
                        launcher_main(
                            &mut tcp_conn,
                            client_info,
                            kill_receiver.clone(),
                            &mut http_msg_rx,
                        )
                        .await
                    }
                    Ok(p) => error!("Unknown packet! {:?}", p),
                    Err(e) => error!("{}", e),
                }
            }
            Err(e) => error!("Error accepting incoming launcher connection: {}", e),
        }
    }

    // Start the UDP listener for the client - launcher connection
    // let udp_bind_addr = format!("127.0.0.1:{}", config.networking.launcher_port);
    // let mut launcher_listener = UdpListener::<ClientPacket>::bind(&udp_bind_addr)
    //     .await
    //     .map_err(|e| error!("{}", e))
    //     .expect("Failed to bind to UDP address!");
    // loop {
    //     info!(
    //         "Waiting for game to connect on port {}!",
    //         config.networking.launcher_port
    //     );
    //     match launcher_listener.wait_for_packet().await {
    //         Ok((packet, local_addr)) => match packet {
    //             ClientPacket::ClientInfo(client_info) => {
    //                 launcher_main(
    //                     &mut launcher_listener,
    //                     local_addr,
    //                     client_info,
    //                     kill_receiver.clone(),
    //                     &mut http_msg_rx,
    //                 )
    //                 .await
    //             }
    //             _ => error!("Unknown packet sent! Packet: {:?}", packet),
    //         },
    //         Err(e) => error!("Failed to receive packet from client: {}", e),
    //     }
    // }
}

async fn launcher_main(
    launcher_socket: &mut TcpConnection<ClientPacket>,
    client_info: launcher_client::handshake::ClientInfoPacket,
    signal_rx: SignalReceiver,
    http_msg_rx: &mut mpsc::Receiver<http::Message>,
) {
    info!("Game connected!");

    if let Err(e) = launcher_socket
        .write_packet(&ClientPacket::Version(
            launcher_client::handshake::VersionPacket {
                protocol_version: 1,
            },
        ))
        .await
    {
        error!("{}", e);
        return;
    }

    // Load the stored auth code (if possible)
    let auth_token = std::fs::read_to_string("auth_token").ok();

    // Load potential auth data
    let mut user_info = if let Some(auth_token) = auth_token {
        trace!("Player has an auth token ready!");

        match http::request_user_data(auth_token).await {
            Ok(user_info) => Some(user_info),
            Err(e) => {
                error!("{}", e);
                None
            }
        }
    } else {
        None
    };
    if user_info.is_none() {
        user_info = {
            trace!("Player is not currently logged in!");

            // AuthenticationInfo packet
            if let Err(e) = launcher_socket
                .write_packet(&ClientPacket::AuthenticationInfo(
                    launcher_client::handshake::AuthenticationInfoPacket {
                        success: false,
                        player_name: String::new(),
                        steam_id: String::new(),
                        avatar_hash: String::new(),
                    },
                ))
                .await
            {
                error!("{}", e);
                return;
            }

            // Wait for LoginRequest packet
            match launcher_socket.wait_for_packet().await {
                Ok(ClientPacket::LoginRequest) => {} // Life is good
                Ok(packet) => {
                    error!(
                        "Unknown packet received, expected Packet::LoginRequest, got {:?}",
                        packet
                    );
                    return;
                }
                Err(e) => {
                    error!("Failed to receive packet from client: {}", e);
                    return;
                }
            }

            if let Err(e) = open::that("http://localhost:4434/login") {
                error!("Failed to open link: {}", e);
                info!("Please open the link manually: http://localhost:4434/login");
            }

            // Now we wait for the login data from the http thread
            match http_msg_rx.recv().await {
                None => {
                    error!("The messaging channel from the http server has closed! Unrecoverable error :(");
                    return;
                }
                Some(msg) => {
                    match msg {
                        http::Message::AuthData(v) => Some(v),
                        http::Message::AuthFailure => {
                            // TODO: We should simply retry the authentication if this fails
                            error!("Failed to authenticate, currently unrecoverable.");
                            return;
                        }
                        _ => {
                            error!("Unsupported message sent, uh oh!");
                            return;
                        }
                    }
                }
            }
        };
    }
    // Now we can unwrap because we have data in here now or have returned to the main menu
    let user_info = user_info.unwrap();
    trace!("Logged in: {:#?}", user_info);

    // Now we can cache the auth token to disk
    if let Err(e) = std::fs::write("auth_token", &user_info.auth) {
        error!("{}", e);
    }

    // AuthenticationInfo packet
    launcher_socket
        .write_packet(&ClientPacket::AuthenticationInfo(
            launcher_client::handshake::AuthenticationInfoPacket {
                success: true,
                player_name: user_info.user.name,
                steam_id: user_info.steam_id.to_string(),
                avatar_hash: user_info.user.avatar_hash,
            },
        ))
        .await;

    loop {
        let auth_token = user_info.auth.clone();
        match launcher_socket.wait_for_packet().await {
            Ok(packet) => match packet {
                ClientPacket::ReloadLauncherConnection => {
                    info!("Client mod disconnected! Waiting for it to reconnect...");
                    break;
                }
                ClientPacket::JoinServer(p) => {
                    trace!("ip: {}", p.ip_address);
                    match p.ip_address.parse::<SocketAddr>() {
                        Ok(server_addr) => {
                            if let Err(e) = launcher_on_server_main(
                                launcher_socket,
                                &client_info,
                                signal_rx.clone(),
                                server_addr,
                                auth_token,
                            )
                            .await
                            {
                                error!("Error in server conn: {}", e);
                                if let Err(e) = launcher_socket
                                    .write_packet(&ClientPacket::ConnectionError(
                                        launcher_client::generic::ConnectionErrorPacket {
                                            error: format!("{}", e),
                                        },
                                    ))
                                    .await
                                {
                                    error!("{}", e);
                                }
                            }
                            // When this returns, we are done playing on the server, so we can
                            // just break and reconnect to the game.
                            break;
                        }
                        Err(e) => {
                            error!("{}", e);
                            if let Err(e) = launcher_socket
                                .write_packet(&ClientPacket::ConnectionError(
                                    launcher_client::generic::ConnectionErrorPacket {
                                        error: format!("{}", e),
                                    },
                                ))
                                .await
                            {
                                error!("{}", e);
                            }
                        }
                    }
                }
                _ => error!("Unknown packet! {:#?}", packet),
            },
            Err(e) => {
                error!("Failed to receive packet from client: {}", e);
                break;
            }
        }
    }
}
