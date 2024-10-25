use std::net::SocketAddr;

use thiserror::Error;

use ngmp_protocol_impl::{
    connection::*,
    launcher_client,
    server_launcher,
};

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

/// Handles the "connecting to server" part of the logic
pub async fn launcher_on_server_main(
    launcher_socket: &mut UdpListener<ClientPacket>,
    local_addr: SocketAddr,
    client_info: &launcher_client::handshake::ClientInfoPacket,
    signal_rx: SignalReceiver,
    server_addr: SocketAddr
) -> anyhow::Result<()> {
    info!("Connecting to server {}...", server_addr);

    // Create TCP connection to the server
    let mut server_tcp_conn = TcpConnection::from_stream(tokio::net::TcpStream::connect(server_addr).await?);

    // Version packet
    server_tcp_conn.write_packet(&ServerPacket::Version(server_launcher::handshake::VersionPacket {
        confirm_id: 1,
        client_version: 1,
    })).await?;

    // Confirmation
    let packet = server_tcp_conn.wait_for_packet().await?;
    match packet {
        ServerPacket::Confirmation(p) => {}, // TODO: Verify confirm id
        _ => {
            error!("Unknown packet: {:?}", packet);
            return Err(ConnectionError::InvalidPacket(packet).into());
        },
    }

    // Authentication packet
    server_tcp_conn.write_packet(&ServerPacket::Authentication(server_launcher::handshake::AuthenticationPacket {
        confirm_id: 2,
        auth_code: "not a real auth code lol".to_string(),
    })).await?;

    // Confirmation
    let packet = server_tcp_conn.wait_for_packet().await?;
    match packet {
        ServerPacket::Confirmation(p) => {}, // TODO: Verify confirm id
        _ => {
            error!("Unknown packet: {:?}", packet);
            return Err(ConnectionError::InvalidPacket(packet).into());
        },
    }

    // Server info packet
    let packet = server_tcp_conn.wait_for_packet().await?;
    let server_info = match packet {
        ServerPacket::ServerInfo(p) => p,
        _ => {
            error!("Unknown packet: {:?}", packet);
            return Err(ConnectionError::InvalidPacket(packet).into());
        },
    };
    debug!("server_info: {:?}", server_info);

    // Start UDP connection
    let udp_addr = format!("0.0.0.0:{}", server_info.udp_port + 1);
    let udp_socket = tokio::net::UdpSocket::bind(&udp_addr).await?;
    let mut udp_remote_addr = server_addr.clone();
    udp_remote_addr.set_port(server_info.udp_port);
    let server_udp_conn: UdpClient<ServerPacket> = UdpClient::connect(udp_socket, udp_remote_addr).await?;

    // Load map packet
    let packet = server_tcp_conn.wait_for_packet().await?;
    let map_info = match packet {
        ServerPacket::LoadMap(p) => p,
        _ => {
            error!("Unknown packet: {:?}", packet);
            return Err(ConnectionError::InvalidPacket(packet).into());
        },
    };
    debug!("map_name: {}", map_info.map_name);
    launcher_socket.write_packet(local_addr, ClientPacket::LoadMap(launcher_client::generic::LoadMapPacket {
        confirm_id: 1, // TODO: Generate confirm ID
        map_name: map_info.map_name,
    })).await?;

    // Wait for confirmation (so we know the map is loaded)
    let (packet, _) = launcher_socket.wait_for_packet().await?;
    match packet {
        ClientPacket::Confirmation(p) => {}, // TODO: Check confirm id of previous packet,
        _ => {
            error!("Unknown packet: {:?}", packet);
            return Err(ConnectionError::InvalidClientPacket(packet).into());
        },
    }

    // Confirmation
    server_tcp_conn.write_packet(&ServerPacket::Confirmation(server_launcher::generic::ConfirmationPacket {
        confirm_id: map_info.confirm_id,
    })).await?;

    launcher_on_server_inner(launcher_socket, local_addr, server_tcp_conn, server_udp_conn).await;

    info!("Done playing on the server :)");

    Ok(())
}

async fn launcher_on_server_inner(
    launcher_socket: &mut UdpListener<ClientPacket>,
    local_addr: SocketAddr,
    mut server_tcp_conn: TcpConnection<ServerPacket>,
    mut server_udp_conn: UdpClient<ServerPacket>,
) {
    loop {
        let mut client_packet = None;
        let mut server_packet = None;
        tokio::select!(
            client_packet_maybe = launcher_socket.wait_for_packet() => {
                trace!("client packet: {:?}", client_packet_maybe);
                if let Err(e) = client_packet_maybe {
                    error!("{}", e);
                    break; // We have run into an error, simply disconnect for now
                }
                let (packet, _) = client_packet_maybe.unwrap();
                client_packet = Some(packet);
            },
            tcp_server_packet_maybe = server_tcp_conn.wait_for_packet() => {
                trace!("server tcp packet: {:?}", tcp_server_packet_maybe);
                if let Err(e) = tcp_server_packet_maybe {
                    error!("{}", e);
                    break; // We have run into an error, simply disconnect for now
                }
                let packet = tcp_server_packet_maybe.unwrap();
                server_packet = Some(packet);
            },
            udp_server_packet_maybe = server_udp_conn.wait_for_packet() => {
                trace!("server udp packet: {:?}", udp_server_packet_maybe);
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
                ClientPacket::VehicleSpawn(p) => server_tcp_conn.write_packet(&ServerPacket::VehicleSpawn(server_launcher::gameplay::VehicleSpawnPacket {
                    confirm_id: p.confirm_id,
                    raw_json: p.raw_json,
                })).await,
                ClientPacket::VehicleDelete(p) => server_tcp_conn.write_packet(&ServerPacket::VehicleDelete(server_launcher::gameplay::VehicleDeletePacket {
                    player_id: p.player_id,
                    vehicle_id: p.vehicle_id,
                })).await,
                _ => {
                    warn!("Unsupported packet: {:?}", packet);
                    Ok(())
                },
            } {
                error!("{}", e);
                break; // We have run into an error, simply disconnect for now
            }
        }

        if let Some(packet) = server_packet {
            if let Err(e) = match packet {
                ServerPacket::VehicleSpawn(p) => launcher_socket.write_packet(local_addr, ClientPacket::VehicleSpawn(launcher_client::gameplay::VehicleSpawnPacket {
                    confirm_id: p.confirm_id,
                    raw_json: p.raw_json,
                })).await,
                ServerPacket::VehicleConfirm(p) => launcher_socket.write_packet(local_addr, ClientPacket::VehicleConfirm(launcher_client::gameplay::VehicleConfirmPacket {
                    confirm_id: p.confirm_id,
                    vehicle_id: p.vehicle_id,
                })).await,
                ServerPacket::VehicleDelete(p) => launcher_socket.write_packet(local_addr, ClientPacket::VehicleDelete(launcher_client::gameplay::VehicleDeletePacket {
                    player_id: p.player_id,
                    vehicle_id: p.vehicle_id,
                })).await,
                _ => {
                    warn!("Unsupported packet: {:?}", packet);
                    Ok(())
                },
            } {
                error!("{}", e);
                break; // We have run into an error, simply disconnect for now
            }
        }
    }
}
