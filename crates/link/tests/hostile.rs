//! Bytes from a hostile or broken peer. Whatever arrives, the link answers
//! with an error and never panics (a panic would take down the connection
//! task, or with `panic = "abort"` on a platform, the whole process), and
//! the size limits hold at their exact edges.

use dent_link::{Control, Hello, Initiator, Keys, Opened, Responder, Transport};

fn hello() -> Hello {
    Hello {
        name: "x".into(),
        proto_min: 1,
        proto: 1,
        probe: false,
    }
}

fn pair() -> (Transport, Transport) {
    let (c, d) = (Keys::generate().unwrap(), Keys::generate().unwrap());
    let (ini, m1) = Initiator::start(&c).unwrap();
    let (res, m2) = Responder::accept(&d, &m1, &hello()).unwrap();
    let (ct, m3) = ini.read(&m2, None).unwrap().finish(&hello()).unwrap();
    let (dt, _) = res.finish(&m3).unwrap();
    (ct, dt)
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}

#[test]
fn garbage_at_every_step_is_an_error_never_a_panic() {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let keys = Keys::generate().unwrap();
    let sizes = [
        0usize, 1, 15, 16, 17, 31, 32, 33, 48, 96, 1000, 65_535, 65_536, 70_000,
    ];
    for &n in &sizes {
        for _ in 0..4 {
            let junk = rng.bytes(n);
            // As message 1, to a daemon.
            let _ = Responder::accept(&keys, &junk, &hello());
            // As message 2, to a client.
            let (ini, _) = Initiator::start(&keys).unwrap();
            let _ = ini.read(&junk, None);
            // As message 3, to a daemon mid-handshake.
            let (ini, m1) = Initiator::start(&Keys::generate().unwrap()).unwrap();
            let (res, _) = Responder::accept(&keys, &m1, &hello()).unwrap();
            drop(ini);
            assert!(res.finish(&junk).is_err());
            // As a frame on a live link.
            let (_, mut dt) = pair();
            assert!(
                dt.open(&junk).is_err(),
                "{n} random bytes opened as a frame"
            );
        }
    }
}

#[test]
fn a_hello_that_isnt_one_is_refused() {
    // Valid Noise carrying a payload that isn't a hello, or a huge one.
    let (c, d) = (Keys::generate().unwrap(), Keys::generate().unwrap());
    for payload in [b"not json".to_vec(), b"{}".to_vec(), vec![b' '; 60_000]] {
        let (ini, m1) = Initiator::start(&c).unwrap();
        let mut hs = snow_responder(&d);
        let mut buf = vec![0u8; 65_535];
        hs.read_message(&m1, &mut buf).unwrap();
        let n = hs.write_message(&payload, &mut buf).unwrap();
        assert!(ini.read(&buf[..n], None).is_err());
    }
}

fn snow_responder(k: &Keys) -> snow::HandshakeState {
    snow::Builder::new(dent_link::noise::PATTERN.parse().unwrap())
        .local_private_key(&k.private)
        .build_responder()
        .unwrap()
}

#[test]
fn size_limits_hold_at_their_exact_edges() {
    let (mut ct, mut dt) = pair();
    let open_all = |frames: Vec<Vec<u8>>, dt: &mut Transport| {
        let mut last = None;
        for f in frames {
            match dt.open(&f) {
                Ok(Some(o)) => last = Some(o),
                Ok(None) => {}
                Err(_) => return None,
            }
        }
        last
    };
    // Exactly the untrusted limit arrives; one byte more doesn't.
    let at = vec![b'a'; dent_link::SMALL];
    assert_eq!(
        open_all(ct.seal_data(&at).unwrap(), &mut dt),
        Some(Opened::Data(at))
    );
    let (mut ct, mut dt) = pair();
    assert_eq!(
        open_all(
            ct.seal_data(&vec![b'a'; dent_link::SMALL + 1]).unwrap(),
            &mut dt
        ),
        None
    );
    // Frames at, under and over one part.
    let (mut ct, mut dt) = pair();
    for n in [
        dent_link::noise::PART - 1,
        dent_link::noise::PART,
        dent_link::noise::PART + 1,
    ] {
        let msg = vec![b'p'; n];
        let frames = ct.seal_data(&msg).unwrap();
        assert!(
            frames.iter().all(|f| f.len() <= 65_535),
            "a frame over Noise's limit"
        );
        assert_eq!(open_all(frames, &mut dt), Some(Opened::Data(msg)));
    }
    // A control message too big for one frame is refused when sealed, not
    // sent broken.
    let big = Control::PairConfirm {
        mac: "f".repeat(70_000),
    };
    assert!(ct.seal_control(&big).is_err());
    // And the link is still in step afterwards.
    let f = ct.seal_control(&Control::Ping { t: 1 }).unwrap();
    assert_eq!(
        dt.open(&f).unwrap(),
        Some(Opened::Control(Control::Ping { t: 1 }))
    );
}
