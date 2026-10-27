//! The relay's bandwidth cap (NETPLAY-10E), on the virtual clock: a
//! direction with a [`LinkRate`] serialises datagrams through a drop-tail
//! link. Raw sockets stand in for the lane so the arithmetic is exact.

use std::net::UdpSocket;
use std::thread;
use std::time::{Duration, Instant};

use crate::{LinkDir, LinkRate, UdpImpair, UdpImpairProxy};

/// A datagram whose wire size (payload + 28) is exactly 1000 bytes.
const PAYLOAD: usize = 972;

struct Pipe {
    proxy: UdpImpairProxy,
    client: UdpSocket,
    host: UdpSocket,
}

fn pipe(rate: LinkRate) -> Pipe {
    let host = UdpSocket::bind("127.0.0.1:0").unwrap();
    host.set_nonblocking(true).unwrap();
    let proxy = UdpImpairProxy::loopback_manual(host.local_addr().unwrap(), 7).unwrap();
    proxy.set(
        LinkDir::Up,
        UdpImpair {
            rate: Some(rate),
            ..UdpImpair::default()
        },
    );
    let client = UdpSocket::bind("127.0.0.1:0").unwrap();
    client.connect(proxy.route().dial).unwrap();
    Pipe {
        proxy,
        client,
        host,
    }
}

impl Pipe {
    /// Send `n` datagrams and wait until the relay has read them all.
    fn offer(&self, n: u64) {
        let base = self.proxy.stats(LinkDir::Up);
        let seen = base.link.frames_in + base.link.dropped;
        for _ in 0..n {
            self.client.send(&[0u8; PAYLOAD]).unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let s = self.proxy.stats(LinkDir::Up);
            if s.link.frames_in + s.link.dropped >= seen + n {
                return;
            }
            assert!(Instant::now() < deadline, "the relay never read {n}");
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// Datagrams the host has received so far (drained).
    fn arrived(&self) -> u64 {
        let mut buf = [0u8; 2048];
        let mut n = 0;
        // The relay forwards on its own thread; give the loopback a beat.
        thread::sleep(Duration::from_millis(30));
        while self.host.recv(&mut buf).is_ok() {
            n += 1;
        }
        n
    }
}

#[test]
fn a_capped_link_releases_one_datagram_per_serialisation_time() {
    // 100 kB/s: a 1000 B datagram takes 10 ms on the link.
    let p = pipe(LinkRate {
        bytes_per_sec: 100_000,
        buffer_bytes: 100_000,
    });
    p.offer(10);
    assert!(p.proxy.advance(Duration::from_millis(5)));
    assert_eq!(p.arrived(), 0, "none has finished serialising");
    assert!(p.proxy.advance(Duration::from_millis(10)));
    assert_eq!(p.arrived(), 1, "the first leaves at 10 ms");
    assert!(p.proxy.advance(Duration::from_millis(30)));
    assert_eq!(p.arrived(), 3, "one more every 10 ms");
    assert!(p.proxy.advance(Duration::from_millis(100)));
    assert_eq!(p.arrived(), 6, "all ten by 100 ms");
    let s = p.proxy.stats(LinkDir::Up);
    assert_eq!(s.rate_dropped, 0);
}

#[test]
fn a_full_buffer_drops_the_tail_and_counts_it() {
    // 100 kB/s with room for 3 datagrams on the link, the one in service
    // included: three fit, the other seven drop.
    let p = pipe(LinkRate {
        bytes_per_sec: 100_000,
        buffer_bytes: 3_000,
    });
    p.offer(10);
    let s = p.proxy.stats(LinkDir::Up);
    assert_eq!(s.rate_dropped, 7, "{s:?}");
    assert_eq!(s.link.dropped, 7);
    assert!(p.proxy.advance(Duration::from_secs(1)));
    assert_eq!(p.arrived(), 3);
}

#[test]
fn an_idle_link_forgets_its_backlog() {
    let p = pipe(LinkRate {
        bytes_per_sec: 100_000,
        buffer_bytes: 4_000,
    });
    p.offer(4);
    assert!(p.proxy.advance(Duration::from_secs(1)));
    assert_eq!(p.arrived(), 4);
    p.offer(4);
    assert_eq!(p.proxy.stats(LinkDir::Up).rate_dropped, 0);
    assert!(p.proxy.advance(Duration::from_secs(1)));
    assert_eq!(p.arrived(), 4);
}

#[test]
fn the_kbit_constructor_is_bits_not_bytes() {
    let r = LinkRate::kbit(256, Duration::from_millis(150));
    assert_eq!(r.bytes_per_sec, 32_000);
    assert_eq!(r.buffer_bytes, 4_800);
}
