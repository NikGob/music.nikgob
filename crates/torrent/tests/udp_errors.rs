//! The DHT and uTP sockets must outlive per-datagram receive errors.
//!
//! On Windows, `recv_from` on a UDP socket fails with WSAECONNRESET (10054)
//! when an earlier `send_to` got an ICMP port unreachable back, and with
//! WSAEMSGSIZE (10040) for a datagram longer than the buffer; the socket keeps
//! working. librqbit 9.0.1 took either as fatal, so DHT and uTP stopped for
//! the whole session (vendor/librqbit-dualstack-sockets/README.nikgob.md).
//! On other platforms an unconnected UDP socket reports neither, so these
//! pass there either way. All traffic stays on localhost.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use librqbit::dht::{Dht, DhtConfig, DhtState};
use librqbit_utp::UtpSocketUdp;
use tokio::net::UdpSocket;
use tokio::time::timeout;

fn localhost() -> SocketAddr {
    (Ipv4Addr::LOCALHOST, 0).into()
}

/// A localhost port nothing listens on once the probe socket is dropped, so
/// whatever is sent there gets an ICMP port unreachable back.
fn closed_port() -> SocketAddr {
    std::net::UdpSocket::bind(localhost())
        .unwrap()
        .local_addr()
        .unwrap()
}

async fn start_dht(bootstrap: SocketAddr) -> Dht {
    DhtState::with_config(DhtConfig {
        listen_addr: Some(localhost()),
        bootstrap_addrs: Some(vec![bootstrap.to_string()]),
        ..Default::default()
    })
    .await
    .unwrap()
}

/// Sends the DHT a ping and waits for any reply from it.
async fn assert_dht_answers(dht: &Dht) {
    let client = UdpSocket::bind(localhost()).await.unwrap();
    client
        .send_to(
            b"d1:ad2:id20:abcdefghij0123456789e1:q4:ping1:t2:aa1:y1:qe",
            dht.listen_addr(),
        )
        .await
        .unwrap();
    let mut buf = [0u8; 1024];
    timeout(Duration::from_secs(5), async {
        loop {
            if let Ok((_, from)) = client.recv_from(&mut buf).await
                && from == dht.listen_addr()
            {
                return;
            }
        }
    })
    .await
    .expect("the DHT stopped answering");
}

#[tokio::test]
async fn dht_answers_after_icmp_port_unreachable() {
    let dht = start_dht(closed_port()).await;
    // Let the bootstrap query reach the closed port and the ICMP error come back.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_dht_answers(&dht).await;
}

#[tokio::test]
async fn dht_answers_after_oversized_datagram() {
    // Stays open, so the bootstrap query gets neither an answer nor an ICMP error.
    let silent = UdpSocket::bind(localhost()).await.unwrap();
    let dht = start_dht(silent.local_addr().unwrap()).await;
    // Longer than the DHT's 16 KiB buffer; some platforms refuse to send it, which is fine.
    let _ = silent.send_to(&[0u8; 20_000], dht.listen_addr()).await;
    assert_dht_answers(&dht).await;
}

async fn assert_utp_connects(client: &Arc<UtpSocketUdp>, server: &Arc<UtpSocketUdp>) {
    timeout(Duration::from_secs(5), async {
        tokio::try_join!(server.accept(), client.connect(server.bind_addr()))
    })
    .await
    .expect("timed out connecting")
    .expect("the uTP socket stopped working");
}

#[tokio::test]
async fn utp_connects_after_icmp_port_unreachable() {
    let client = UtpSocketUdp::new_udp(localhost()).await.unwrap();
    let server = UtpSocketUdp::new_udp(localhost()).await.unwrap();
    // The SYN to the closed port gets an ICMP port unreachable back.
    let _ = timeout(Duration::from_millis(500), client.connect(closed_port())).await;
    assert_utp_connects(&client, &server).await;
}
