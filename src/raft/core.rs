extern crate log;
extern crate simplelog;
use crate::raft::types::{
    Leader, LogEntry, Peer, RpcClient, Server, State, VoteRequest, VoteResponse,
};
use log::info;
use math::round;
use rand::Rng;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

pub fn start_server(
    server: Arc<Mutex<Server>>,
    rpc_client: impl RpcClient + std::marker::Send + 'static,
) {
    server.lock().unwrap().start();

    let background_task_handle = thread::spawn(move || {
        background_task(server, &rpc_client);
    });

    background_task_handle.join().unwrap();
}

pub fn handle_vote_request(server: Arc<Mutex<Server>>, request: VoteRequest) -> VoteResponse {
    let mut tmp_server = server.lock().unwrap();

    match tmp_server.voted_for {
        Some(_) => VoteResponse {
            term: request.term,
            vote_granted: false,
        },
        None => {
            if request.term > tmp_server.term {
                tmp_server.voted_for = Some(Peer {
                    id: request.candidate_id,
                    // Fake address for now.
                    address: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 7879),
                });

                VoteResponse {
                    term: request.term,
                    vote_granted: true,
                }
            } else {
                VoteResponse {
                    term: request.term,
                    vote_granted: false,
                }
            }
        }
    }
}

pub fn handle_log_entry(server: Arc<Mutex<Server>>, entry: LogEntry) -> u64 {
    let mut server = server.lock().unwrap();

    if let LogEntry::Heartbeat { term, peer_id } = entry {
        info!(
            "Server {} with term {}, received heartbeat from {} with term {}",
            server.id, server.term, peer_id, term
        );

        server.refresh_timeout();

        if term > server.term {
            info!(
                "Server {} becoming follower. The new leader is: {}",
                server.id, peer_id
            );

            server.term = term;
            server.state = State::FOLLOWER;
            server.voted_for = None;
            server.current_leader = Some(Leader {
                id: peer_id.to_string(),
                term: term,
            })
        }
    };

    let current_term = server.term;

    current_term
}

fn background_task(server: Arc<Mutex<Server>>, rpc_client: &impl RpcClient) {
    loop {
        handle_timeout(Arc::clone(&server), rpc_client);
        broadcast_heartbeat(Arc::clone(&server), rpc_client);
    }
}

fn broadcast_heartbeat(server: Arc<Mutex<Server>>, rpc_client: &impl RpcClient) {
    let is_leader = server.lock().unwrap().state == State::LEADER;

    if is_leader {
        let term = server.lock().unwrap().term;
        let id = server.lock().unwrap().id.to_string();

        rpc_client.broadcast_log_entry(LogEntry::Heartbeat {
            term: term,
            peer_id: id,
        });

        // A touch of randomness, so that we can get the chance
        // to have other leader elections.
        let mut rng = rand::thread_rng();
        thread::sleep(Duration::new(rng.gen_range(1..7), 0));
    }
}

fn handle_timeout(server: Arc<Mutex<Server>>, rpc_client: &impl RpcClient) {
    let server_id = server.lock().unwrap().id.to_string();
    let has_timed_out = server.lock().unwrap().has_timed_out();

    if has_timed_out {
        info!("Server {} has timed out.", server_id);

        new_election(Arc::clone(&server), rpc_client);
    }
}

fn new_election(server: Arc<Mutex<Server>>, rpc_client: &impl RpcClient) {
    let vote_request = prepare_vote_request(Arc::clone(&server));
    let server_id = server.lock().unwrap().id.to_string();
    let server_current_term = server.lock().unwrap().term;

    info!(
        "Server {}, with term {}, started the election process.",
        server_id, server_current_term
    );

    let vote_response = match vote_request {
        Some(request) => Some(rpc_client.request_vote(request)),
        None => None,
    };

    if let Some(r) = vote_response {
        let own_election;
        {
            let mut server = server.lock().unwrap();
            own_election = has_won_the_election(&server, r) && !server.has_timed_out();
        }

        if own_election {
            become_leader(Arc::clone(&server), rpc_client);
        }
    }
}

fn prepare_vote_request(server: Arc<Mutex<Server>>) -> Option<VoteRequest> {
    if server.lock().unwrap().state == State::LEADER {
        return None;
    }

    {
        let mut server_tmp = server.lock().unwrap();
        server_tmp.state = State::CANDIDATE;
        server_tmp.term = server_tmp.term + 1;
        server_tmp.refresh_timeout();
        server_tmp.voted_for = Some(Peer {
            id: server_tmp.id.to_string(),
            address: server_tmp.address,
        });
    }

    let new_term = server.lock().unwrap().term;
    let id = server.lock().unwrap().id.to_string();

    Some(VoteRequest {
        term: new_term,
        candidate_id: id,
    })
}

fn has_won_the_election(server: &Server, response: Vec<VoteResponse>) -> bool {
    let number_of_servers = server.number_of_peers + 1; // All peers + current server

    let votes = response.iter().filter(|r| r.vote_granted).count();

    let min_quorum = round::floor((number_of_servers / 2) as f64, 0);

    (votes + 1) > min_quorum as usize && State::CANDIDATE == server.state
}

fn become_leader(server: Arc<Mutex<Server>>, rpc_client: &impl RpcClient) {
    let mut server = server.lock().unwrap();

    server.become_leader();

    let log_entry = LogEntry::Heartbeat {
        term: server.term,
        peer_id: server.id.to_string(),
    };

    rpc_client.broadcast_log_entry(log_entry);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raft::types::ServerConfig;
    use std::net::{Ipv4Addr, SocketAddrV4};
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    #[test]
    fn raft_new_election() {
        // When the server gets the vote from its peers
        let server = Arc::new(Mutex::new(build_server()));
        let rpc_client = FakeRpc {
            granted_vote: true,
            sleeps_for: Duration::new(0, 0),
            peers: create_peers(2),
        };

        new_election(Arc::clone(&server), &rpc_client);

        {
            let tmp_server = server.lock().unwrap();
            assert_eq!(tmp_server.state, State::LEADER);
            assert_eq!(tmp_server.term, 1);
        }

        // When the server does not get the vote from its peers
        let server = Arc::new(Mutex::new(build_server()));
        let rpc_client = FakeRpc {
            granted_vote: false,
            sleeps_for: Duration::new(0, 0),
            peers: create_peers(2),
        };

        new_election(Arc::clone(&server), &rpc_client);

        {
            let tmp_server = server.lock().unwrap();
            assert_eq!(tmp_server.state, State::CANDIDATE);
            assert_eq!(tmp_server.term, 1);
        }

        // When the server is alredy leader.
        let server = Arc::new(Mutex::new(build_server()));
        let rpc_client = FakeRpc {
            granted_vote: false,
            sleeps_for: Duration::new(0, 0),
            peers: create_peers(2),
        };

        server.lock().unwrap().state = State::LEADER;
        server.lock().unwrap().term = 10;

        new_election(Arc::clone(&server), &rpc_client);

        {
            let tmp_server = server.lock().unwrap();
            assert_eq!(tmp_server.state, State::LEADER);
            // term does not change
            assert_eq!(tmp_server.term, 10);
        }

        // When the server times out again, it should not
        // become leader even when getting votes.
        let server = Arc::new(Mutex::new(build_server()));
        let rpc_client = FakeRpc {
            granted_vote: true,
            sleeps_for: Duration::new(1, 0),
            peers: create_peers(2),
        };

        server.lock().unwrap().start();

        new_election(Arc::clone(&server), &rpc_client);

        {
            let tmp_server = server.lock().unwrap();
            assert_eq!(tmp_server.state, State::CANDIDATE);
            assert_eq!(tmp_server.term, 1);
        }
    }

    #[test]
    fn raft_handle_log_entry() {
        // When the heartbeat contains a higher term
        let server = Arc::new(Mutex::new(build_server()));
        server.lock().unwrap().term = 10;

        let log_entry = LogEntry::Heartbeat {
            term: 19,
            peer_id: "server_3".to_string(),
        };

        server.lock().unwrap().start();

        handle_log_entry(Arc::clone(&server), log_entry);

        {
            let tmp_server = server.lock().unwrap();
            assert_eq!(tmp_server.state, State::FOLLOWER);
            assert_eq!(tmp_server.term, 19);
            assert!(tmp_server.next_timeout.as_ref().unwrap() > &Instant::now());
        }

        // When the heartbeat contains a higher term
        // and the current server is a Leader, then it
        // becomes a follower.
        let server = Arc::new(Mutex::new(build_server()));
        server.lock().unwrap().state = State::LEADER;
        server.lock().unwrap().term = 10;

        let log_entry = LogEntry::Heartbeat {
            term: 19,
            peer_id: "server_3".to_string(),
        };

        server.lock().unwrap().start();

        handle_log_entry(Arc::clone(&server), log_entry);

        {
            let tmp_server = server.lock().unwrap();
            assert_eq!(tmp_server.state, State::FOLLOWER);
            assert_eq!(tmp_server.term, 19);
            assert!(tmp_server.next_timeout.as_ref().unwrap() > &Instant::now());
        }
    }

    #[test]
    fn raft_handle_vote_request() {
        let server = Arc::new(Mutex::new(build_server()));
        server.lock().unwrap().start();

        let candidate_id = "server_2";

        let vote_request = VoteRequest {
            candidate_id: candidate_id.to_string(),
            term: 1,
        };

        let vote_response = handle_vote_request(Arc::clone(&server), vote_request);

        assert!(vote_response.vote_granted);
        {
            let tmp_server = server.lock().unwrap();
            assert_eq!(vote_response.term, tmp_server.term + 1);
            assert_eq!(
                tmp_server.voted_for.as_ref().unwrap().id,
                candidate_id.to_string()
            );
        }

        // Now the server has already voted for that term
        let new_candidate_id = "server_3";

        let vote_request = VoteRequest {
            candidate_id: new_candidate_id.to_string(),
            term: 1,
        };

        let vote_response = handle_vote_request(Arc::clone(&server), vote_request);

        assert!(!vote_response.vote_granted);
        {
            let tmp_server = server.lock().unwrap();
            assert_ne!(
                tmp_server.voted_for.as_ref().unwrap().id,
                new_candidate_id.to_string()
            );
            assert_eq!(vote_response.term, tmp_server.term + 1);
        }

        // When the server did not vote yet, but the candidate's term is the same
        // as the current server.
        server.lock().unwrap().voted_for = None;

        let another_candidate_id = "server_4";

        let vote_request = VoteRequest {
            candidate_id: another_candidate_id.to_string(),
            term: server.lock().unwrap().term,
        };

        let vote_response = handle_vote_request(Arc::clone(&server), vote_request);

        {
            let tmp_server = server.lock().unwrap();
            assert!(tmp_server.voted_for.as_ref().is_none());
            assert_eq!(vote_response.term, tmp_server.term);
            assert!(!vote_response.vote_granted);
        }
    }

    fn build_server() -> Server {
        let config = ServerConfig {
            timeout: Duration::new(1, 0),
        };

        let number_of_peers = 2;
        let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 9090);
        let id = "server_1".to_string();

        Server::new(config, number_of_peers, address, id)
    }

    fn create_peers(n: usize) -> Vec<Peer> {
        let mut peers = Vec::new();

        for i in 0..n {
            peers.push(Peer {
                id: i.to_string(),
                address: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 9090),
            });
        }

        peers
    }

    struct FakeRpc {
        granted_vote: bool,
        sleeps_for: Duration,
        peers: Vec<Peer>,
    }

    impl RpcClient for FakeRpc {
        fn request_vote(&self, request: VoteRequest) -> Vec<VoteResponse> {
            let mut response = Vec::new();

            for _peer in self.peers.iter() {
                response.push(VoteResponse {
                    term: request.term,
                    vote_granted: self.granted_vote,
                });
            }
            sleep(self.sleeps_for);
            response
        }

        fn broadcast_log_entry(&self, _log_entry: LogEntry) {
            info!("broadcast");
        }
    }
}
