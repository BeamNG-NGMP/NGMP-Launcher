use std::net::SocketAddr;

use thiserror::Error;

use serde::{Deserialize, Serialize};

use ngmp_protocol_impl::{connection::*, launcher_client, server_launcher};

use ngmp_protocol_impl::launcher_client::Packet as ClientPacket;
use ngmp_protocol_impl::server_launcher::Packet as ServerPacket;

use crate::signal::SignalReceiver;

#[derive(Error, Debug)]
pub enum ConnectionError {
    #[error("invalid packet sent: {0:?}")]
    InvalidPacket(ServerPacket),
    #[error("invalid client packet sent: {0:?}")]
    InvalidClientPacket(ClientPacket),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct VehicleTransformData {
    // m
    pos: [f32; 3],
    // help
    rot: [f32; 4],
    // m/s
    vel: [f32; 3],
    // rad/s
    rvel: [f32; 3],
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct VehicleTransformWithFutureData {
    pos: [f32; 3],
    rot: [f32; 4],
    vel: [f32; 3],
    rvel: [f32; 3],

    fut_pos: [f32; 3],
    fut_rot: [f32; 4],
    fut_vel: [f32; 3],
    fut_rvel: [f32; 3],
}

/// Handles the "connecting to server" part of the logic
pub async fn launcher_on_server_main(
    launcher_socket: &mut TcpConnection<ClientPacket>,
    client_info: &launcher_client::handshake::ClientInfoPacket,
    signal_rx: SignalReceiver,
    server_addr: SocketAddr,
    auth_token: String,
) -> anyhow::Result<()> {
    info!("Connecting to server {}...", server_addr);

    // Create TCP connection to the server
    let mut server_tcp_conn =
        TcpConnection::from_stream(tokio::net::TcpStream::connect(server_addr).await?);

    // Version packet
    server_tcp_conn
        .write_packet(&ServerPacket::Version(
            server_launcher::handshake::VersionPacket {
                confirm_id: 1,
                client_version: 1,
            },
        ))
        .await?;

    // Confirmation
    let packet = server_tcp_conn.wait_for_packet().await?;
    match packet {
        ServerPacket::Confirmation(p) => {} // TODO: Verify confirm id
        _ => {
            error!("Unknown packet: {:?}", packet);
            return Err(ConnectionError::InvalidPacket(packet).into());
        }
    }

    // Authentication packet
    server_tcp_conn
        .write_packet(&ServerPacket::Authentication(
            server_launcher::handshake::AuthenticationPacket {
                confirm_id: 2,
                auth_code: auth_token,
            },
        ))
        .await?;

    // Confirmation
    let packet = server_tcp_conn.wait_for_packet().await?;
    match packet {
        ServerPacket::Confirmation(p) => {} // TODO: Verify confirm id
        _ => {
            error!("Unknown packet: {:?}", packet);
            return Err(ConnectionError::InvalidPacket(packet).into());
        }
    }

    // Server info packet
    let packet = server_tcp_conn.wait_for_packet().await?;
    let server_info = match packet {
        ServerPacket::ServerInfo(p) => p,
        _ => {
            error!("Unknown packet: {:?}", packet);
            return Err(ConnectionError::InvalidPacket(packet).into());
        }
    };
    debug!("server_info: {:?}", server_info);

    // Start UDP connection
    let udp_addr = format!("0.0.0.0:{}", server_info.udp_port + 1);
    let udp_socket = tokio::net::UdpSocket::bind(&udp_addr).await?;
    let mut udp_remote_addr = server_addr.clone();
    udp_remote_addr.set_port(server_info.udp_port);
    let server_udp_conn: UdpClient<ServerPacket> =
        UdpClient::connect(udp_socket, udp_remote_addr).await?;

    // Load map packet
    let packet = server_tcp_conn.wait_for_packet().await?;
    let map_info = match packet {
        ServerPacket::LoadMap(p) => p,
        _ => {
            error!("Unknown packet: {:?}", packet);
            return Err(ConnectionError::InvalidPacket(packet).into());
        }
    };
    debug!("map_name: {}", map_info.map_name);
    launcher_socket
        .write_packet(&ClientPacket::LoadMap(
            launcher_client::generic::LoadMapPacket {
                confirm_id: 1, // TODO: Generate confirm ID
                map_string: map_info.map_name,
            },
        ))
        .await?;

    // Wait for confirmation (so we know the map is loaded)
    let packet = launcher_socket.wait_for_packet().await?;
    match packet {
        ClientPacket::Confirmation(p) => {} // TODO: Check confirm id of previous packet,
        _ => {
            error!("Unknown packet: {:?}", packet);
            return Err(ConnectionError::InvalidClientPacket(packet).into());
        }
    }

    // Confirmation
    server_tcp_conn
        .write_packet(&ServerPacket::Confirmation(
            server_launcher::generic::ConfirmationPacket {
                confirm_id: map_info.confirm_id,
            },
        ))
        .await?;

    launcher_on_server_inner(launcher_socket, server_tcp_conn, server_udp_conn).await;

    info!("Done playing on the server :)");

    Ok(())
}

async fn launcher_on_server_inner(
    launcher_socket: &mut TcpConnection<ClientPacket>,
    mut server_tcp_conn: TcpConnection<ServerPacket>,
    mut server_udp_conn: UdpClient<ServerPacket>,
) {
    let conn_start = std::time::Instant::now();

    loop {
        let mut client_packet = None;
        let mut server_packet = None;
        tokio::select!(
            client_packet_maybe = launcher_socket.wait_for_packet() => {
                if let Err(e) = client_packet_maybe {
                    error!("{}", e);
                    break; // We have run into an error, simply disconnect for now
                }
                let packet = client_packet_maybe.unwrap();
                client_packet = Some(packet);
            },
            tcp_server_packet_maybe = server_tcp_conn.wait_for_packet() => {
                if let Err(e) = tcp_server_packet_maybe {
                    error!("{}", e);
                    break; // We have run into an error, simply disconnect for now
                }
                let packet = tcp_server_packet_maybe.unwrap();
                server_packet = Some(packet);
            },
            udp_server_packet_maybe = server_udp_conn.wait_for_packet() => {
                if let Err(e) = udp_server_packet_maybe {
                    error!("{}", e);
                    break; // We have run into an error, simply disconnect for now
                }
                let packet = udp_server_packet_maybe.unwrap();
                server_packet = Some(packet);
            }
        );

        if let Some(packet) = client_packet {
            if let Err(e) = match packet {
                ClientPacket::VehicleSpawn(p) => match p.steam_id.parse::<u64>() {
                    Ok(steam_id) => {
                        server_tcp_conn
                            .write_packet(&ServerPacket::VehicleSpawn(
                                server_launcher::gameplay::VehicleSpawnPacket {
                                    confirm_id: p.confirm_id,
                                    steam_id,
                                    vehicle_id: p.vehicle_id,
                                    vehicle_data: server_launcher::gameplay::VehicleData {
                                        jbeam: p.vehicle_data.jbeam,
                                        object_id: p.vehicle_data.object_id,
                                        paints: p.vehicle_data.paints,
                                        part_config: p.vehicle_data.part_config,
                                        pos: p.vehicle_data.pos,
                                        rot: p.vehicle_data.rot,
                                    },
                                },
                            ))
                            .await
                    }
                    Err(e) => Err(e.into()),
                },
                ClientPacket::VehicleDelete(p) => match p.steam_id.parse::<u64>() {
                    Ok(player_id) => {
                        server_tcp_conn
                            .write_packet(&ServerPacket::VehicleDelete(
                                server_launcher::gameplay::VehicleDeletePacket {
                                    player_id,
                                    vehicle_id: p.vehicle_id,
                                },
                            ))
                            .await
                    }
                    Err(e) => Err(e.into()),
                },
                ClientPacket::VehicleTransform(p) => match p.steam_id.parse::<u64>() {
                    Ok(player_id) => {
                        if let Ok(mut map) = serde_json::from_str::<
                            std::collections::HashMap<String, serde_json::Value>,
                        >(&p.transform)
                        {
                            map.insert(
                                String::from("ms"),
                                serde_json::Value::Number(
                                    (conn_start.elapsed().as_millis() as u64).into(),
                                ),
                            );
                            trace!("transform outgoing");
                            server_udp_conn
                                .write_packet(ServerPacket::VehicleTransform(
                                    server_launcher::gameplay::VehicleTransformPacket {
                                        player_id,
                                        vehicle_id: p.vehicle_id,
                                        transform: serde_json::to_string(&map).unwrap(),
                                    },
                                ))
                                .await
                        } else {
                            error!("Error decoding VehicleTransform transform json");
                            Ok(())
                        }
                    }
                    Err(e) => Err(e.into()),
                },
                ClientPacket::VehicleUpdate(p) => match p.steam_id.parse::<u64>() {
                    Ok(player_id) => {
                        trace!("update outgoing");
                        server_udp_conn
                            .write_packet(ServerPacket::VehicleUpdate(
                                server_launcher::gameplay::VehicleUpdatePacket {
                                    player_id,
                                    vehicle_id: p.vehicle_id,
                                    ms: conn_start.elapsed().as_millis() as u32,
                                    runtime_data: p.runtime_data,
                                },
                            ))
                            .await
                    }
                    Err(e) => Err(e.into()),
                },
                _ => {
                    warn!("Unsupported packet: {:?}", packet);
                    Ok(())
                }
            } {
                error!("{}", e);
                break; // We have run into an error, simply disconnect for now
            }
        }

        if let Some(packet) = server_packet {
            if let Err(e) = match packet {
                ServerPacket::PlayerData(p) => {
                    launcher_socket
                        .write_packet(&ClientPacket::PlayerData(
                            launcher_client::gameplay::PlayerDataPacket {
                                players: p
                                    .players
                                    .into_iter()
                                    .map(|p| launcher_client::gameplay::PlayerData {
                                        name: p.name,
                                        steam_id: p.steam_id.to_string(),
                                        avatar_hash: p.avatar_hash,
                                    })
                                    .collect(),
                            },
                        ))
                        .await
                }
                ServerPacket::VehicleSpawn(p) => {
                    launcher_socket
                        .write_packet(&ClientPacket::VehicleSpawn(
                            launcher_client::gameplay::VehicleSpawnPacket {
                                confirm_id: p.confirm_id,
                                steam_id: p.steam_id.to_string(),
                                vehicle_id: p.vehicle_id,
                                vehicle_data: launcher_client::gameplay::VehicleData {
                                    jbeam: p.vehicle_data.jbeam,
                                    object_id: p.vehicle_data.object_id,
                                    paints: p.vehicle_data.paints,
                                    part_config: p.vehicle_data.part_config,
                                    pos: p.vehicle_data.pos,
                                    rot: p.vehicle_data.rot,
                                },
                            },
                        ))
                        .await
                }
                ServerPacket::VehicleConfirm(p) => {
                    launcher_socket
                        .write_packet(&ClientPacket::VehicleConfirm(
                            launcher_client::gameplay::VehicleConfirmPacket {
                                confirm_id: p.confirm_id,
                                vehicle_id: p.vehicle_id,
                                object_id: p.obj_id,
                            },
                        ))
                        .await
                }
                ServerPacket::VehicleDelete(p) => {
                    launcher_socket
                        .write_packet(&ClientPacket::VehicleDelete(
                            launcher_client::gameplay::VehicleDeletePacket {
                                steam_id: p.player_id.to_string(),
                                vehicle_id: p.vehicle_id,
                            },
                        ))
                        .await
                }
                ServerPacket::VehicleTransform(p) => {
                    trace!("transform incoming");
                    if let Ok(parsed) = serde_json::from_str::<VehicleTransformData>(&p.transform) {
                        let predicted = predict_future(parsed, 20.0);

                        launcher_socket
                            .write_packet(&ClientPacket::VehicleTransform(
                                launcher_client::gameplay::VehicleTransformPacket {
                                    steam_id: p.player_id.to_string(),
                                    vehicle_id: p.vehicle_id,
                                    transform: serde_json::to_string(&predicted).unwrap(),
                                },
                            ))
                            .await
                    } else {
                        error!("Discarding packet, failed to deserialize transform data");
                        Ok(())
                    }
                }
                ServerPacket::VehicleUpdate(p) => {
                    trace!("update incoming");
                    launcher_socket
                        .write_packet(&ClientPacket::VehicleUpdate(
                            launcher_client::gameplay::VehicleUpdatePacket {
                                steam_id: p.player_id.to_string(),
                                vehicle_id: p.vehicle_id,
                                runtime_data: p.runtime_data,
                            },
                        ))
                        .await
                }
                _ => {
                    warn!("Unsupported packet: {:?}", packet);
                    Ok(())
                }
            } {
                error!("{}", e);
                break; // We have run into an error, simply disconnect for now
            }
        }
    }
}

fn predict_future(now: VehicleTransformData, ms: f32) -> VehicleTransformWithFutureData {
    let s = ms / 1000f32;
    VehicleTransformWithFutureData {
        pos: now.pos,
        rot: now.rot,
        vel: now.vel,
        rvel: now.rvel,

        fut_pos: [
            now.pos[0] + now.vel[0] * s,
            now.pos[1] + now.vel[1] * s,
            now.pos[2] + now.vel[2] * s,
        ],
        fut_rot: now.rot,
        // Velocity can only be predicted if we also know the drag on the vehicle!
        fut_vel: now.vel,
        fut_rvel: now.rvel,
    }
}
