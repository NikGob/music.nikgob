# librqbit-dualstack-sockets 0.7.0 (patched)

Copy of librqbit-dualstack-sockets 0.7.0 from crates.io (Apache-2.0, Igor Katson) with one change in
`src/socket.rs`: `UdpSocket::recv_from` skips the errors Windows reports for a single datagram
(WSAECONNRESET 10054 after an ICMP port unreachable for an earlier send, WSAENETRESET 10052 after an
ICMP time exceeded, WSAEMSGSIZE 10040 for a datagram longer than the buffer) and keeps reading.
Upstream returns them, and librqbit's DHT framer and uTP dispatcher stop on any receive error, so one
unreachable DHT node ended DHT and uTP for the whole session ("dht finished with error: framer
failed", "uTP: dispatcher dead"). Fixing it here covers both, plus the UDP tracker client and UPnP
discovery, which read through the same socket. Wired in through `[patch.crates-io]` in the
workspace Cargo.toml; `crates/torrent/tests/udp_errors.rs` checks the behaviour. Besides that,
`src/bind_device.rs` names an unused Windows parameter `_name`: a path dependency's warnings are not
capped like a registry one's, and this one would show in every build.

Upstream fixed the same thing at the two call sites (ikatson/rqbit#690 in librqbit-dht,
ikatson/librqbit-utp#4), merged 2026-09-28 but not released yet. Drop this copy once a librqbit
release after 9.0.1 and a librqbit-utp release after 0.7.0 are in Cargo.lock.
