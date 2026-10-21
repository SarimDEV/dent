//! Waking a sleeping machine on this device's network: a magic packet (six
//! 0xff bytes, then its card's address sixteen times) to each card's
//! subnet broadcast and to the local broadcast, on the usual ports. It
//! works only from the same network, and only on a machine set to wake for
//! it (on a Mac: Wake for network access).

use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};

use dent_protocol::Wake;

/// The packet that wakes the card with address `mac` (`aa:bb:cc:dd:ee:ff`).
pub fn packet(mac: &str) -> Option<[u8; 102]> {
    let bytes: Vec<u8> = mac
        .split([':', '-'])
        .map(|h| u8::from_str_radix(h, 16).ok())
        .collect::<Option<_>>()?;
    if bytes.len() != 6 {
        return None;
    }
    let mut p = [0xffu8; 102];
    for i in 0..16 {
        p[6 + i * 6..12 + i * 6].copy_from_slice(&bytes);
    }
    Some(p)
}

/// Send the packets that wake a machine with these cards. How many went;
/// an error only when none could.
pub fn wake(targets: &[Wake]) -> std::io::Result<usize> {
    let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    sock.set_broadcast(true)?;
    let mut sent = 0;
    let mut last = None;
    for t in targets {
        let Some(p) = packet(&t.mac) else {
            continue;
        };
        let mut to: Vec<Ipv4Addr> = vec![Ipv4Addr::BROADCAST];
        if let Ok(b) = t.broadcast.parse() {
            to.insert(0, b);
        }
        for ip in to {
            for port in [9, 7] {
                match sock.send_to(&p, SocketAddrV4::new(ip, port)) {
                    Ok(_) => sent += 1,
                    Err(e) => last = Some(e),
                }
            }
        }
    }
    match (sent, last) {
        (0, Some(e)) => Err(e),
        _ => Ok(sent),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_magic_packet_names_the_card_sixteen_times() {
        let p = packet("aa:bb:cc:dd:ee:0f").unwrap();
        assert_eq!(&p[..6], &[0xff; 6]);
        assert_eq!(&p[6..12], &[0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0x0f]);
        assert_eq!(&p[96..], &[0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0x0f]);
        assert!(packet("aa:bb").is_none());
        assert!(packet("zz:bb:cc:dd:ee:ff").is_none());
    }

    #[test]
    fn the_packet_arrives() {
        // Sent to this machine's own loopback, where a listener stands in
        // for the sleeping card.
        let l = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let s = UdpSocket::bind("127.0.0.1:0").unwrap();
        s.send_to(&packet("01:02:03:04:05:06").unwrap(), ("127.0.0.1", port))
            .unwrap();
        let mut buf = [0u8; 200];
        let (n, _) = l.recv_from(&mut buf).unwrap();
        assert_eq!(n, 102);
        assert_eq!(&buf[6..12], &[1, 2, 3, 4, 5, 6]);
    }
}
