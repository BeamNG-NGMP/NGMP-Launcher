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
    {
        tokio::select!(
            client_packet_maybe = launcher_socket.wait_for_packet() => {
                debug!("client packet: {:?}", client_packet_maybe);
            },
            tcp_server_packet_maybe = server_tcp_conn.wait_for_packet() => {
                debug!("server tcp packet: {:?}", tcp_server_packet_maybe);
            },
            udp_server_packet_maybe = server_udp_conn.wait_for_packet() => {
                debug!("server udp packet: {:?}", udp_server_packet_maybe);
            }
        );

        if let Err(e) = server_tcp_conn.write_packet(&ServerPacket::Confirmation(server_launcher::generic::ConfirmationPacket {
            confirm_id: 5,
        })).await {
            error!("{}", e);
        }
        // debug!("server tcp packet: {:?}", server_tcp_conn.wait_for_packet().await);
    }
}
