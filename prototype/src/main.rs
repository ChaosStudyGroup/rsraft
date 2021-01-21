mod raft;

use raft::core;
use raft::core::{RpcClient, RpcServer, Server};
use raft::tcp_rpc_client::TcpRpcClient;
use raft::tcp_rpc_server::TcpRpcServer;
use rand::{thread_rng, Rng};
use std::convert::TryInto;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

fn main() {
    let mut rpc_servers = Vec::new();

    let address_1 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 3300);
    let server_1 = Arc::new(Mutex::new(Server::new(
        core::ServerConfig {
            timeout: Duration::new(5, 0),
        },
        2,
        address_1,
        "server_1".to_string(),
    )));
    let address_1_peers = vec![
        core::Peer {
            id: "server_2".to_string(),
            address: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 3301),
        },
        core::Peer {
            id: "server_3".to_string(),
            address: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 3302),
        },
    ];

    rpc_servers.push(TcpRpcServer::new(Arc::clone(&server_1), address_1));

    let address_2 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 3301);
    let server_2 = Arc::new(Mutex::new(Server::new(
        core::ServerConfig {
            timeout: Duration::new(6, 0),
        },
        2,
        address_2,
        "server_2".to_string(),
    )));
    let address_2_peers = vec![
        core::Peer {
            id: "server_1".to_string(),
            address: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 3300),
        },
        core::Peer {
            id: "server_3".to_string(),
            address: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 3302),
        },
    ];

    rpc_servers.push(TcpRpcServer::new(Arc::clone(&server_2), address_2));

    let address_3 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 3302);
    let server_3 = Arc::new(Mutex::new(Server::new(
        core::ServerConfig {
            timeout: Duration::new(7, 0),
        },
        2,
        address_3,
        "server_3".to_string(),
    )));
    let address_3_peers = vec![
        core::Peer {
            id: "server_1".to_string(),
            address: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 3300),
        },
        core::Peer {
            id: "server_3".to_string(),
            address: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 3301),
        },
    ];

    rpc_servers.push(TcpRpcServer::new(Arc::clone(&server_3), address_3));

    let mut server_threads = Vec::new();
    for rpc_server in rpc_servers {
        server_threads.push(thread::spawn(move || {
            rpc_server.start_server();
        }));
    }

    thread::sleep(Duration::new(1, 0));
    let mut raft_servers_threads = Vec::new();

    raft_servers_threads.push(thread::spawn(move || {
        let client = TcpRpcClient::new(&address_1_peers);

        core::start_server(Arc::clone(&server_1), client);
    }));

    raft_servers_threads.push(thread::spawn(move || {
        let client = TcpRpcClient::new(&address_2_peers);

        core::start_server(Arc::clone(&server_2), client);
    }));

    raft_servers_threads.push(thread::spawn(move || {
        let client = TcpRpcClient::new(&address_3_peers);

        core::start_server(Arc::clone(&server_3), client);
    }));

    for st in server_threads {
        st.join().unwrap();
    }

    for rs in raft_servers_threads {
        rs.join().unwrap();
    }
}